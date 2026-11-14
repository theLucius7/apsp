use crate::{
    error::{ApiError, ApiResult},
    text::Text,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssetSettings {
    pub region: String,
    pub group_name: String,
    pub tags: Vec<String>,
    pub hidden: bool,
    pub offline_notify: bool,
    pub agent_mirror: String,
    pub price: Option<String>,
    pub currency: String,
    pub billing_cycle: u16,
    pub expires_at: Option<i64>,
    pub auto_renewal: bool,
    pub traffic_limit: String,
    pub traffic_limit_type: TrafficMode,
    pub reset_day: u8,
    pub network_interface: String,
}

/// The billing cycle in days of a server whose cycle was never saved; the ledger and the
/// server's settings both read it.
const DEFAULT_BILLING_CYCLE: u16 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficMode {
    #[default]
    Sum,
    Max,
    Min,
    Up,
    Down,
}

impl TrafficMode {
    pub fn used(self, up: u128, down: u128) -> u128 {
        match self {
            Self::Sum => up.saturating_add(down),
            Self::Max => up.max(down),
            Self::Min => up.min(down),
            Self::Up => up,
            Self::Down => down,
        }
    }
}

impl Default for AssetSettings {
    fn default() -> Self {
        Self {
            region: String::new(),
            group_name: String::new(),
            tags: Vec::new(),
            hidden: false,
            offline_notify: true,
            agent_mirror: String::new(),
            price: None,
            currency: "CNY".into(),
            billing_cycle: DEFAULT_BILLING_CYCLE,
            expires_at: None,
            auto_renewal: false,
            traffic_limit: "0".into(),
            traffic_limit_type: TrafficMode::Sum,
            reset_day: 1,
            network_interface: String::new(),
        }
    }
}

impl AssetSettings {
    pub fn normalized(mut self) -> ApiResult<Self> {
        self.agent_mirror = self.agent_mirror.trim().trim_end_matches('/').into();
        if !self.agent_mirror.is_empty() {
            let valid = reqwest::Url::parse(&self.agent_mirror)
                .ok()
                .is_some_and(|url| {
                    url.scheme() == "https"
                        && url.port_or_known_default() == Some(443)
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.query().is_none()
                        && url.fragment().is_none()
                        && url
                            .host_str()
                            .is_some_and(|host| host != "localhost" && url.domain().is_some())
                });
            if !valid || !Text::bytes(512).accepts(&self.agent_mirror) {
                return Err(ApiError::BadRequest(
                    "下载加速需为不含凭据、查询参数或片段的 HTTPS 镜像前缀".into(),
                ));
            }
        }
        self.region = self.region.trim().to_uppercase();
        self.group_name = self.group_name.trim().into();
        if !Text::chars(16).accepts(&self.region)
            || !Text::chars(40).accepts(&self.group_name)
            || self.tags.len() > 16
        {
            return Err(ApiError::BadRequest(
                "地区最多 16 字、分组最多 40 字、标签最多 16 个".into(),
            ));
        }
        let mut tags = Vec::new();
        for tag in self.tags {
            let tag = tag.trim().to_string();
            if !Text::chars(32).not_empty().accepts(&tag) {
                return Err(ApiError::BadRequest("每个标签需为 1–32 个字符".into()));
            }
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        self.tags = tags;
        self.price = self
            .price
            .map(|price| normalize_price(&price))
            .transpose()?;
        self.currency = self.currency.trim().to_ascii_uppercase();
        if !apsp_panel_host::money::currency(&self.currency) {
            return Err(ApiError::BadRequest(
                "币种需为三位字母代码，如 CNY、USD".into(),
            ));
        }
        if self.billing_cycle > 3650
            || self
                .expires_at
                .is_some_and(|at| !(0..=253_402_300_799).contains(&at))
            || (self.auto_renewal && (self.billing_cycle == 0 || self.expires_at.is_none()))
        {
            return Err(ApiError::BadRequest(
                "费用周期需为 0–3650 天；自动顺延需设置到期日期和非零周期".into(),
            ));
        }
        self.traffic_limit = self.traffic_limit.trim().into();
        if self.traffic_limit.is_empty()
            || !self.traffic_limit.bytes().all(|b| b.is_ascii_digit())
            || self.traffic_limit.parse::<u64>().is_err()
            || !(1..=31).contains(&self.reset_day)
        {
            return Err(ApiError::BadRequest(
                "流量额度需为非负整数字节且不超过 2^64−1，重置日需为 1–31".into(),
            ));
        }
        self.traffic_limit = self.traffic_limit.parse::<u64>().unwrap().to_string();
        let patterns: Vec<_> = self
            .network_interface
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if patterns.len() > 16
            || patterns
                .iter()
                .any(|s| !Text::chars(64).accepts(s) || *s == "!")
        {
            return Err(ApiError::BadRequest(
                "统计网卡最多 16 个匹配项，每项不超过 64 字，排除项不能为空".into(),
            ));
        }
        self.network_interface = patterns.join(",");
        Ok(self)
    }

    pub fn renew(&mut self, now: i64) {
        if let Some(expiry) = self.expires_at
            && self.auto_renewal
            && self.billing_cycle > 0
            && expiry <= now
        {
            let cycle = i64::from(self.billing_cycle) * 86_400;
            let next = expiry + ((now - expiry) / cycle + 1) * cycle;
            if next <= 253_402_300_799 {
                self.expires_at = Some(next);
            }
        }
    }

    /// Whether the interface counts for the server's traffic ([`includes_interface`]).
    pub fn includes(&self, interface: &str) -> bool {
        includes_interface(&self.network_interface, interface)
    }
}

/// Whether `interface` counts for a server's traffic under its saved `network_interface`
/// patterns, as the asset settings promise: comma-separated, `*` matches any run of
/// characters, a leading `!` excludes and exclusions win; without an inclusion pattern every
/// interface counts. The server view, period traffic, alerts and statistics all ask this.
pub fn includes_interface(patterns: &str, interface: &str) -> bool {
    let patterns: Vec<_> = patterns.split(',').filter(|p| !p.is_empty()).collect();
    let included = !patterns.iter().any(|p| !p.starts_with('!'))
        || patterns
            .iter()
            .any(|p| !p.starts_with('!') && matches(p, interface));
    included
        && !patterns
            .iter()
            .any(|p| p.strip_prefix('!').is_some_and(|p| matches(p, interface)))
}

fn matches(pattern: &str, value: &str) -> bool {
    let (p, v) = (pattern.as_bytes(), value.as_bytes());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < v.len() {
        if i < p.len() && p[i] == b'*' {
            star = Some(i);
            i += 1;
            mark = j;
        } else if i < p.len() && p[i] == v[j] {
            i += 1;
            j += 1;
        } else if let Some(s) = star {
            mark += 1;
            j = mark;
            i = s + 1;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

fn normalize_price(value: &str) -> ApiResult<String> {
    let invalid =
        || ApiError::BadRequest("金额需为 0–1000000000，最多两位小数；留空表示未填写".into());
    let (whole, fraction) = apsp_panel_host::money::decimal(value.trim(), 2).ok_or_else(invalid)?;
    let whole: u64 = whole.parse().map_err(|_| invalid())?;
    let fraction: u64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().map_err(|_| invalid())? * if fraction.len() == 1 { 10 } else { 1 }
    };
    if whole > 1_000_000_000 || (whole == 1_000_000_000 && fraction != 0) {
        return Err(invalid());
    }
    Ok(format!("{whole}.{fraction:02}"))
}

/// SQL over the asset settings of a `servers` row, for filters and columns that stay in the
/// database (counts, joins, ordered pages). `server` names the row: its alias in the
/// caller's query, or `servers`. Each expression reads its key the way [`AssetSettings`]
/// does, so a key that was never saved has the typed default, except where a function says
/// it returns the saved value.
pub mod sql {
    /// Whether the server is hidden from public views; never NULL.
    pub fn hidden(server: &str) -> String {
        format!("(COALESCE({server}.asset_settings->>'hidden','false')='true')")
    }

    /// Whether offline alerts are sent for the server; never NULL.
    pub fn offline_notify(server: &str) -> String {
        format!("(COALESCE({server}.asset_settings->>'offline_notify','true')<>'false')")
    }

    /// The interfaces that count for the server's traffic, as saved (comma-separated
    /// patterns, matched with [`super::includes_interface`]); empty when not saved.
    pub fn network_interface(server: &str) -> String {
        format!("COALESCE({server}.asset_settings->>'network_interface','')")
    }

    /// The price as saved (a decimal string), NULL when none is.
    pub fn price(server: &str) -> String {
        format!("{server}.asset_settings->>'price'")
    }

    /// The three-letter currency code.
    pub fn currency(server: &str) -> String {
        format!("COALESCE({server}.asset_settings->>'currency','CNY')")
    }

    /// The billing cycle in days, 0 for a one-off purchase; never NULL.
    pub fn billing_cycle(server: &str) -> String {
        format!(
            "COALESCE(({server}.asset_settings->>'billing_cycle')::int,{})",
            super::DEFAULT_BILLING_CYCLE
        )
    }

    /// Whether the expiry renews by itself; never NULL.
    pub fn auto_renewal(server: &str) -> String {
        format!("(COALESCE({server}.asset_settings->>'auto_renewal','false')='true')")
    }

    /// Whether an expiry time is saved.
    pub fn has_expiry(server: &str) -> String {
        format!("(jsonb_typeof({server}.asset_settings->'expires_at')='number')")
    }

    /// The expiry time in seconds, NULL when none is saved.
    pub fn expires_at(server: &str) -> String {
        format!("({server}.asset_settings->>'expires_at')::bigint")
    }
}

pub async fn renew_due(pool: &PgPool, now: i64) -> anyhow::Result<()> {
    let rows: Vec<(i64, serde_json::Value)> = sqlx::query_as(&format!(
        "SELECT id, asset_settings FROM servers WHERE deleted_at IS NULL AND {} AND {}<=$1 LIMIT 500",
        sql::auto_renewal("servers"),
        sql::expires_at("servers")
    ))
    .bind(now)
    .fetch_all(pool)
    .await?;
    for (id, value) in rows {
        let mut asset: AssetSettings = serde_json::from_value(value.clone())?;
        let previous = asset.expires_at;
        asset.renew(now);
        if asset.expires_at != previous {
            sqlx::query("UPDATE servers SET asset_settings=$3 WHERE id=$1 AND asset_settings=$2 AND deleted_at IS NULL")
                .bind(id).bind(value).bind(serde_json::to_value(asset)?).execute(pool).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::includes_interface;

    #[test]
    fn interface_patterns_match_as_the_settings_describe() {
        assert!(includes_interface("", "eth0"));
        assert!(includes_interface("!lo", "eth0") && !includes_interface("!lo", "lo"));
        assert!(includes_interface("eth*,!eth1", "eth0"));
        assert!(!includes_interface("eth*,!eth1", "eth1"));
        assert!(!includes_interface("eth*,!eth1", "wg0"));
        assert!(includes_interface("en*s*", "enp0s3") && !includes_interface("eth0", "eth01"));
    }
}

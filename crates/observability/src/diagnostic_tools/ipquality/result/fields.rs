use super::receipts::bounded_text;
use crate::ip_quality::{QualityField, QualityFieldKind};
use serde_json::Value;

fn meaningful(text: &str) -> bool {
    bounded_text(text, 512)
        && !matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "null" | "unknown" | "undefined" | "n/a" | "nan" | "none" | "-" | "未知"
        )
}

fn field(raw: &Value, pointer: &str, label: &str, kind: QualityFieldKind) -> Option<QualityField> {
    let value = raw.pointer(pointer)?;
    let normalized = match kind {
        QualityFieldKind::Text => {
            Value::String(value.as_str().filter(|text| meaningful(text))?.into())
        }
        QualityFieldKind::CountryCode => {
            let text = value.as_str()?;
            if text.len() != 2 || !text.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                return None;
            }
            Value::String(text.to_ascii_uppercase())
        }
        QualityFieldKind::Boolean => Value::Bool(value.as_bool()?),
        QualityFieldKind::Asn => {
            let text = value
                .as_str()?
                .strip_prefix("AS")
                .unwrap_or(value.as_str()?);
            let asn = text.parse::<u32>().ok()?;
            if asn == 0 {
                return None;
            }
            Value::from(asn)
        }
        QualityFieldKind::Score | QualityFieldKind::Latitude | QualityFieldKind::Longitude => {
            let number = value
                .as_f64()
                .or_else(|| value.as_str()?.trim().parse::<f64>().ok())?;
            let valid = number.is_finite()
                && match kind {
                    QualityFieldKind::Score => (0.0..=100.0).contains(&number),
                    QualityFieldKind::Latitude => (-90.0..=90.0).contains(&number),
                    QualityFieldKind::Longitude => (-180.0..=180.0).contains(&number),
                    _ => false,
                };
            if !valid {
                return None;
            }
            Value::from(number)
        }
    };
    Some(QualityField {
        label: label.into(),
        value: normalized,
        kind: Some(kind),
    })
}

fn definitions(dataset: &str) -> Vec<(String, &'static str, QualityFieldKind)> {
    use QualityFieldKind::*;
    let mut result = Vec::new();
    if dataset == "MaxMind" {
        for (pointer, label, kind) in [
            ("/Info/ASN", "ASN", Asn),
            ("/Info/Organization", "网络组织", Text),
            ("/Info/Region/Code", "国家代码", CountryCode),
            ("/Info/City/Name", "城市", Text),
            ("/Info/Latitude", "纬度", Latitude),
            ("/Info/Longitude", "经度", Longitude),
            ("/Info/TimeZone", "时区", Text),
        ] {
            result.push((pointer.into(), label, kind));
        }
    }
    for (key, label, kind) in [
        ("CountryCode", "国家代码", CountryCode),
        ("Proxy", "代理", Boolean),
        ("Tor", "Tor", Boolean),
        ("VPN", "VPN", Boolean),
        ("Server", "数据中心", Boolean),
        ("Abuser", "滥用标记", Boolean),
        ("Robot", "爬虫标记", Boolean),
    ] {
        result.push((format!("/Factor/{key}/{dataset}"), label, kind));
    }
    result.push((format!("/Type/Usage/{dataset}"), "用途类型", Text));
    result.push((format!("/Type/Company/{dataset}"), "组织类型", Text));
    result.push((format!("/Score/{dataset}"), "上游风险评分（原值）", Score));
    if matches!(
        dataset,
        "Netflix" | "Youtube" | "TikTok" | "AmazonPrimeVideo" | "Reddit"
    ) {
        result.push((
            format!("/Media/{dataset}/Status"),
            "可用性（工具推断）",
            Text,
        ));
        result.push((format!("/Media/{dataset}/Region"), "页面地区", CountryCode));
        result.push((
            format!("/Media/{dataset}/Type"),
            "解锁类型（工具推断）",
            Text,
        ));
    }
    result
}

pub(super) fn parse_fields(raw: &Value, dataset: &str) -> Vec<QualityField> {
    definitions(dataset)
        .into_iter()
        .filter_map(|(pointer, label, kind)| field(raw, &pointer, label, kind))
        .collect()
}

pub(crate) fn validate_cached_fields(
    database: &str,
    fields: Vec<QualityField>,
) -> Vec<QualityField> {
    let Some(dataset) = database.strip_prefix("node-") else {
        return Vec::new();
    };
    let definitions = definitions(dataset);
    fields
        .into_iter()
        .filter_map(|saved| {
            let (_, label, kind) = definitions
                .iter()
                .find(|(_, label, _)| *label == saved.label)?;
            // Persisted scalars are already normalized; validate without re-parsing source strings.
            let valid =
                match kind {
                    QualityFieldKind::Text => saved.value.as_str().is_some_and(meaningful),
                    QualityFieldKind::CountryCode => saved.value.as_str().is_some_and(|text| {
                        text.len() == 2 && text.bytes().all(|byte| byte.is_ascii_uppercase())
                    }),
                    QualityFieldKind::Boolean => saved.value.is_boolean(),
                    QualityFieldKind::Asn => saved
                        .value
                        .as_u64()
                        .is_some_and(|asn| asn > 0 && asn <= u32::MAX as u64),
                    QualityFieldKind::Score => saved.value.as_f64().is_some_and(|number| {
                        number.is_finite() && (0.0..=100.0).contains(&number)
                    }),
                    QualityFieldKind::Latitude => saved.value.as_f64().is_some_and(|number| {
                        number.is_finite() && (-90.0..=90.0).contains(&number)
                    }),
                    QualityFieldKind::Longitude => saved.value.as_f64().is_some_and(|number| {
                        number.is_finite() && (-180.0..=180.0).contains(&number)
                    }),
                };
            valid.then(|| QualityField {
                label: (*label).into(),
                value: saved.value,
                kind: Some(*kind),
            })
        })
        .collect()
}

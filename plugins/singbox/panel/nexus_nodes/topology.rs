//! Catalogue topology of Nexus lines, ported from the legacy panel's
//! `NexusNodeTopology` so members see the same routes after migration.
//!
//! This is inference from operator-entered names and tags (`geo:XX`,
//! `provider:Name`, `residential`, `dynamic-residential`, `relay`,
//! `via-node:ID`, `ai-split`, `taiwan-ai-egress`, `apple-news-us-egress`),
//! never traceroute or geolocation. Endpoints are compared but never returned.
use serde::Serialize;

const REGIONS: &[(&str, &[&str])] = &[
    ("HK", &["香港", "hong kong", "🇭🇰"]),
    ("SG", &["新加坡", "singapore", "🇸🇬"]),
    ("JP", &["日本", "japan", "🇯🇵"]),
    ("KR", &["韩国", "韓國", "korea", "🇰🇷"]),
    ("TW", &["台湾", "台灣", "taiwan", "🇹🇼"]),
    ("MY", &["马来西亚", "馬來西亞", "malaysia", "🇲🇾"]),
    ("TH", &["泰国", "泰國", "thailand", "🇹🇭"]),
    ("IN", &["印度", "india", "🇮🇳"]),
    ("AU", &["澳大利亚", "澳大利亞", "australia", "🇦🇺"]),
    ("US", &["美国", "美國", "united states", "🇺🇸"]),
    ("DE", &["德国", "德國", "germany", "🇩🇪"]),
    ("GB", &["英国", "英國", "united kingdom", "🇬🇧"]),
    ("FR", &["法国", "法國", "france", "🇫🇷"]),
    ("NL", &["荷兰", "荷蘭", "netherlands", "🇳🇱"]),
    ("CA", &["加拿大", "canada", "🇨🇦"]),
    ("PH", &["菲律宾", "菲律賓", "philippines", "🇵🇭"]),
    ("ID", &["印度尼西亚", "印尼", "indonesia", "🇮🇩"]),
    ("VN", &["越南", "vietnam", "🇻🇳"]),
    ("BR", &["巴西", "brazil", "🇧🇷"]),
    ("ZA", &["南非", "south africa", "🇿🇦"]),
    ("AE", &["阿联酋", "阿聯酋", "emirates", "🇦🇪"]),
];

/// One catalogue node as the topology needs it.
pub(crate) struct CatalogNode<'a> {
    pub id: i64,
    pub name: &'a str,
    pub tags: &'a [String],
    pub host: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Identity {
    pub region: Option<String>,
    pub provider: Option<String>,
    pub residential: bool,
    pub dynamic: bool,
    pub classification_source: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Point {
    pub id: &'static str,
    pub role: &'static str,
    pub region: Option<String>,
    pub provider: Option<String>,
    pub node_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Edge {
    pub from: &'static str,
    pub to: &'static str,
    pub traffic: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Topology {
    #[serde(flatten)]
    pub identity: Identity,
    pub kind: &'static str,
    pub source: &'static str,
    pub inferred: bool,
    pub position_precision: &'static str,
    pub front_node_id: Option<i64>,
    pub points: Vec<Point>,
    pub edges: Vec<Edge>,
}

fn lowered(tags: &[String]) -> Vec<String> {
    tags.iter().map(|tag| tag.trim().to_lowercase()).collect()
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Removes `[...]` groups (one nested level), as `\[(?:[^\[\]]|\[[^\[\]]*\])*\]`.
fn strip_brackets(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '['
            && let Some(end) = bracket_end(&characters, index)
        {
            index = end + 1;
            continue;
        }
        result.push(characters[index]);
        index += 1;
    }
    result
}

fn bracket_end(characters: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    while index < characters.len() {
        match characters[index] {
            ']' => return Some(index),
            '[' => {
                index += 1;
                while index < characters.len() && characters[index] != ']' {
                    if characters[index] == '[' {
                        return None;
                    }
                    index += 1;
                }
                if index == characters.len() {
                    return None;
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Removes `\s*\((?:hy2|hysteria2?)\)` case-insensitively.
fn strip_hysteria_marks(value: &str) -> String {
    let mut result = value.to_owned();
    for mark in ["(hysteria2)", "(hysteria)", "(hy2)"] {
        // The marks are ASCII, so a byte-wise case-insensitive match always
        // starts and ends on character boundaries.
        while let Some(position) = (0..result.len()).find(|&index| {
            result
                .as_bytes()
                .get(index..index + mark.len())
                .is_some_and(|bytes| bytes.eq_ignore_ascii_case(mark.as_bytes()))
        }) {
            let start = result[..position].trim_end().len();
            result.replace_range(start..position + mark.len(), "");
        }
    }
    result
}

/// Cuts `\s*(?:动态|動態)?家[宽寬].*$`.
fn strip_residential_suffix(value: &str) -> String {
    let Some(position) = ["家宽", "家寬"]
        .iter()
        .filter_map(|mark| value.find(mark))
        .min()
    else {
        return value.to_owned();
    };
    let mut start = position;
    for prefix in ["动态", "動態"] {
        if value[..start].ends_with(prefix) {
            start -= prefix.len();
            break;
        }
    }
    value[..start].trim_end().to_owned()
}

fn relay_name(name: &str) -> bool {
    // `\[[a-z]{2}\[\d+\]\]` case-insensitively.
    let characters: Vec<char> = name.chars().collect();
    (0..characters.len()).any(|start| {
        let at = |offset: usize| characters.get(start + offset).copied();
        if at(0) != Some('[')
            || !at(1).is_some_and(|c| c.is_ascii_alphabetic())
            || !at(2).is_some_and(|c| c.is_ascii_alphabetic())
            || at(3) != Some('[')
        {
            return false;
        }
        let mut offset = 4;
        while at(offset).is_some_and(|c| c.is_ascii_digit()) {
            offset += 1;
        }
        offset > 4 && at(offset) == Some(']') && at(offset + 1) == Some(']')
    })
}

pub(crate) fn identity(name: &str, tags: &[String]) -> Identity {
    let name: String = name.chars().take(1024).collect();
    let lower_tags = lowered(tags);
    let parts: Vec<&str> = name
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let route = parts
        .first()
        .map(|part| part.to_lowercase())
        .unwrap_or_default();
    let mut region = None;
    for tag in &lower_tags {
        if let Some(code) = tag.strip_prefix("geo:")
            && code.len() == 2
            && code.bytes().all(|byte| byte.is_ascii_lowercase())
        {
            region = Some(code.to_uppercase());
        }
    }
    if region.is_none() {
        // A leading flag is unambiguous even for policy routes with several exits.
        region = REGIONS
            .iter()
            .find(|(_, terms)| route.starts_with(terms[terms.len() - 1]))
            .or_else(|| {
                REGIONS
                    .iter()
                    .find(|(_, terms)| terms.iter().any(|term| contains_ci(&route, term)))
            })
            .map(|(code, _)| (*code).to_owned());
    }
    let tag_residential = lower_tags
        .iter()
        .any(|tag| tag == "residential" || tag == "dynamic-residential");
    let name_residential = ["家宽", "家寬"].iter().any(|mark| name.contains(mark))
        || contains_ci(&name, "residential");
    let mut provider = strip_residential_suffix(
        strip_hysteria_marks(&strip_brackets(parts.get(1).copied().unwrap_or(""))).trim(),
    )
    .trim()
    .to_owned();
    if lower_tags.iter().any(|tag| tag == "ai-split")
        && parts.len() < 3
        && provider.contains("分流")
    {
        provider.clear();
    }
    for tag in tags {
        if let Some(value) = tag
            .get(.."provider:".len())
            .filter(|prefix| prefix.eq_ignore_ascii_case("provider:"))
            .and_then(|_| tag.get("provider:".len()..))
            && (1..=64).contains(&value.chars().count())
            && !value.contains('\n')
        {
            provider = value.trim().to_owned();
        }
    }
    Identity {
        region,
        provider: (!provider.is_empty()).then(|| provider.chars().take(64).collect()),
        residential: tag_residential || name_residential,
        dynamic: lower_tags.iter().any(|tag| tag == "dynamic-residential")
            || ["动态家宽", "動態家寬"]
                .iter()
                .any(|mark| name.contains(mark)),
        classification_source: if tag_residential {
            "catalog_tag"
        } else if name_residential {
            "catalog_name"
        } else {
            "node_catalog"
        },
    }
}

/// Whether a node describes a direct entry by itself.
pub(crate) fn direct(node: &CatalogNode<'_>) -> bool {
    let tags = lowered(node.tags);
    !tags
        .iter()
        .any(|tag| tag == "ai-split" || tag == "relay" || tag.starts_with("via-node:"))
        && !relay_name(node.name)
}

/// `ingress` lists nodes that may supply a shared-ingress inference (direct
/// nodes with monitoring); explicit `via-node:ID` tags may name any node.
pub(crate) fn describe(
    node: &CatalogNode<'_>,
    catalog: &[CatalogNode<'_>],
    ingress: &[i64],
) -> Topology {
    let identity = identity(node.name, node.tags);
    let tags = lowered(node.tags);
    let policy = tags.iter().any(|tag| tag == "ai-split");
    let relay_hint = tags.iter().any(|tag| tag == "relay") || relay_name(node.name);
    let mut explicit = None;
    for tag in &tags {
        if let Some(id) = tag.strip_prefix("via-node:")
            && !id.is_empty()
            && id.bytes().all(|byte| byte.is_ascii_digit())
        {
            explicit = id.parse::<i64>().ok();
        }
    }
    let mut front: Option<&CatalogNode<'_>> = None;
    if explicit.is_some() || relay_hint || policy {
        for candidate in catalog {
            if candidate.id == node.id {
                continue;
            }
            if let Some(explicit) = explicit {
                if candidate.id == explicit {
                    front = Some(candidate);
                    break;
                }
                continue;
            }
            if !ingress.contains(&candidate.id) {
                continue;
            }
            let host = node.host.trim();
            if !host.is_empty() && host == candidate.host.trim() {
                front = Some(candidate);
                break;
            }
        }
    }
    let kind = if policy {
        "policy"
    } else if relay_hint || explicit.is_some() {
        "relay"
    } else {
        "direct"
    };
    let source = match (front.is_some(), explicit.is_some(), kind) {
        (true, true, _) => "catalog_tag",
        (true, false, _) => "shared_ingress",
        (false, _, "direct") => "node_catalog",
        _ => "unresolved",
    };
    let point = |id: &'static str, role: &'static str, info: Option<&Identity>, node_id| Point {
        id,
        role,
        region: info.and_then(|info| info.region.clone()),
        provider: info.and_then(|info| info.provider.clone()),
        node_id,
    };
    let mut points = Vec::new();
    let mut edges = Vec::new();
    if kind == "direct" {
        points.push(point("entry", "entry", Some(&identity), Some(node.id)));
    } else {
        let front_identity = front.map(|front| self::identity(front.name, front.tags));
        points.push(point(
            "entry",
            "entry",
            front_identity.as_ref(),
            front.map(|front| front.id),
        ));
        if policy {
            // Policy branches declared by the catalogue, not sequential hops.
            if tags.iter().any(|tag| tag == "taiwan-ai-egress") {
                let provider = (identity.region.as_deref() == Some("TW"))
                    .then(|| identity.provider.clone())
                    .flatten();
                points.push(Point {
                    id: "egress-tw",
                    role: "egress",
                    region: Some("TW".into()),
                    provider,
                    node_id: None,
                });
                edges.push(Edge {
                    from: "entry",
                    to: "egress-tw",
                    traffic: "ai",
                });
            }
            if tags.iter().any(|tag| tag == "apple-news-us-egress") {
                points.push(Point {
                    id: "egress-us",
                    role: "egress",
                    region: Some("US".into()),
                    provider: None,
                    node_id: None,
                });
                edges.push(Edge {
                    from: "entry",
                    to: "egress-us",
                    traffic: "apple_news",
                });
            }
        } else {
            points.push(point("egress", "egress", Some(&identity), Some(node.id)));
            edges.push(Edge {
                from: "entry",
                to: "egress",
                traffic: "all",
            });
        }
    }
    // Inferred relationships stay distinct from explicit via-node tags.
    let inferred = kind != "direct" && (explicit.is_none() || front.is_none());
    Topology {
        identity,
        kind,
        source,
        inferred,
        position_precision: "region",
        front_node_id: front.map(|front| front.id),
        points,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn names_and_tags_yield_region_provider_and_residential_class() {
        let dynamic = identity("🇲🇾 马来西亚 | TMNET || 动态家宽[hk[4]] [25x]", &[]);
        assert_eq!(dynamic.region.as_deref(), Some("MY"));
        assert_eq!(dynamic.provider.as_deref(), Some("TMNET"));
        assert!(dynamic.residential && dynamic.dynamic);
        assert_eq!(dynamic.classification_source, "catalog_name");
        let relay = identity("台湾 | HiNet[hk[1]] [20x]", &[]);
        assert_eq!(relay.region.as_deref(), Some("TW"));
        assert_eq!(relay.provider.as_deref(), Some("HiNet"));
        assert!(!relay.residential);
        let tagged = identity(
            "Somewhere | Vendor (hy2)",
            &tags(&["geo:jp", "Provider:Example Net", "residential"]),
        );
        assert_eq!(tagged.region.as_deref(), Some("JP"));
        assert_eq!(tagged.provider.as_deref(), Some("Example Net"));
        assert_eq!(tagged.classification_source, "catalog_tag");
        assert_eq!(
            identity("Hong Kong | Vendor (Hy2)", &[])
                .provider
                .as_deref(),
            Some("Vendor")
        );
        assert_eq!(identity("plain", &[]).region, None);
    }

    #[test]
    fn relays_take_the_entry_from_tags_or_a_shared_monitored_ingress() {
        let none: Vec<String> = Vec::new();
        let relay_tags = tags(&["relay"]);
        let via = tags(&["via-node:1"]);
        let catalog = [
            CatalogNode {
                id: 1,
                name: "🇭🇰 香港 | HKT",
                tags: &none,
                host: "entry.example.invalid",
            },
            CatalogNode {
                id: 2,
                name: "台湾 | HiNet[hk[1]]",
                tags: &none,
                host: "entry.example.invalid",
            },
            CatalogNode {
                id: 3,
                name: "日本 | IIJ",
                tags: &relay_tags,
                host: "other.example.invalid",
            },
            CatalogNode {
                id: 4,
                name: "美国 | Vendor",
                tags: &via,
                host: "x.example.invalid",
            },
        ];
        let shared = describe(&catalog[1], &catalog, &[1]);
        assert_eq!(
            (shared.kind, shared.source, shared.inferred),
            ("relay", "shared_ingress", true)
        );
        assert_eq!(shared.front_node_id, Some(1));
        assert_eq!(shared.points[0].region.as_deref(), Some("HK"));
        assert_eq!(shared.points[1].region.as_deref(), Some("TW"));
        assert_eq!(describe(&catalog[1], &catalog, &[]).source, "unresolved");
        let unresolved = describe(&catalog[2], &catalog, &[1]);
        assert_eq!((unresolved.kind, unresolved.front_node_id), ("relay", None));
        assert_eq!(unresolved.points[0].region, None);
        let explicit = describe(&catalog[3], &catalog, &[]);
        assert_eq!((explicit.source, explicit.inferred), ("catalog_tag", false));
        let direct = describe(&catalog[0], &catalog, &[1]);
        assert_eq!(
            (direct.kind, direct.points.len(), direct.edges.len()),
            ("direct", 1, 0)
        );
        assert!(super::direct(&catalog[0]) && !super::direct(&catalog[1]));
    }

    #[test]
    fn policy_routes_list_branches_not_sequential_hops() {
        let policy = tags(&["ai-split", "taiwan-ai-egress", "apple-news-us-egress"]);
        let node = CatalogNode {
            id: 9,
            name: "🇭🇰 香港 | 智能分流",
            tags: &policy,
            host: "h",
        };
        let topology = describe(&node, std::slice::from_ref(&node), &[]);
        assert_eq!(topology.kind, "policy");
        assert_eq!(
            topology.identity.provider, None,
            "a split label is not a provider"
        );
        let traffic: Vec<_> = topology.edges.iter().map(|edge| edge.traffic).collect();
        assert_eq!(traffic, ["ai", "apple_news"]);
    }
}

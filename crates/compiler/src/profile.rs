//! Bounded client template references, rules and deterministic group expansion.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn validate_groups(groups: &[Value], name: &str, members: &str) -> Result<(), &'static str> {
    let mut graph = BTreeMap::new();
    for group in groups {
        let tag = group[name]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
            .ok_or("分组或节点名称无效")?;
        if graph.insert(tag, group).is_some() {
            return Err("分组或节点名称重复");
        }
    }
    fn visit<'a>(
        tag: &'a str,
        graph: &BTreeMap<&'a str, &'a Value>,
        members: &str,
        stack: &mut BTreeSet<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> Result<(), &'static str> {
        if done.contains(tag) {
            return Ok(());
        }
        if !stack.insert(tag) {
            return Err("分组引用存在循环");
        }
        let group = graph.get(tag).ok_or("分组引用了未授权或不可用节点")?;
        if let Some(values) = group.get(members) {
            let values = values
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= 1024)
                .ok_or("分组成员列表无效或为空")?;
            for value in values {
                visit(
                    value.as_str().ok_or("分组成员必须是名称")?,
                    graph,
                    members,
                    stack,
                    done,
                )?;
            }
            if let Some(default) = group.get("default")
                && !values.contains(default)
            {
                return Err("默认节点不属于当前分组");
            }
        }
        stack.remove(tag);
        done.insert(tag);
        Ok(())
    }
    let mut done = BTreeSet::new();
    for tag in graph.keys() {
        visit(tag, &graph, members, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

pub fn validate_rule_sets(route: &Value) -> Result<(), &'static str> {
    let mut tags = BTreeSet::new();
    if let Some(sets) = route.get("rule_set") {
        let sets = sets
            .as_array()
            .filter(|v| v.len() <= 64)
            .ok_or("规则集最多 64 项")?;
        for set in sets {
            let object = set.as_object().ok_or("规则集必须是对象")?;
            if object.keys().any(|key| {
                ![
                    "type",
                    "tag",
                    "format",
                    "url",
                    "rules",
                    "update_interval",
                    "http_client",
                ]
                .contains(&key.as_str())
            }) {
                return Err("规则集包含不支持的字段");
            }
            let tag = set["tag"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128)
                .ok_or("规则集标签无效")?;
            if !tags.insert(tag) {
                return Err("规则集标签重复");
            }
            match set["type"].as_str() {
                Some("remote")
                    if set["url"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("https://") && s.len() <= 2048)
                        && matches!(set["format"].as_str(), Some("source" | "binary")) => {}
                Some("inline") if set["rules"].as_array().is_some_and(|s| s.len() <= 256) => {}
                _ => return Err("规则集需为 HTTPS 远程规则集或内联规则集"),
            }
        }
    }
    validate_rule_refs(route.get("rules"), &tags)
}

pub fn validate_rule_refs(
    value: Option<&Value>,
    tags: &BTreeSet<&str>,
) -> Result<(), &'static str> {
    if let Some(value) = value {
        match value {
            Value::Array(values) => {
                for value in values {
                    validate_rule_refs(Some(value), tags)?;
                }
            }
            Value::Object(object) => {
                if let Some(reference) = object.get("rule_set") {
                    let refs: Vec<&Value> = match reference {
                        Value::Array(v) => v.iter().collect(),
                        other => vec![other],
                    };
                    if refs
                        .iter()
                        .any(|v| !v.as_str().is_some_and(|s| tags.contains(s)))
                    {
                        return Err("规则引用不存在的规则集");
                    }
                }
                validate_rule_refs(object.get("rules"), tags)?;
            }
            _ => return Err("规则必须是对象或数组"),
        }
    }
    Ok(())
}

pub fn validate_clash_template(value: &Value) -> Result<(), &'static str> {
    let object = value.as_object().ok_or("Clash 模板必须是 JSON 对象")?;
    if object.keys().any(|key| {
        ![
            "proxy-groups",
            "rules",
            "rule-providers",
            "dns",
            "mode",
            "ipv6",
            "mixed-port",
            "allow-lan",
            "tun",
        ]
        .contains(&key.as_str())
    }) {
        return Err("Clash 模板包含不支持的字段");
    }
    if let Some(groups) = value.get("proxy-groups") {
        for group in groups
            .as_array()
            .filter(|v| v.len() <= 32)
            .ok_or("Clash 分组最多 32 项")?
        {
            let object = group.as_object().ok_or("Clash 分组无效")?;
            if object.keys().any(|k| {
                ![
                    "name",
                    "type",
                    "proxies",
                    "filter",
                    "url",
                    "interval",
                    "tolerance",
                    "lazy",
                    "strategy",
                ]
                .contains(&k.as_str())
            }) {
                return Err("Clash 分组含不支持的字段");
            }
            if !matches!(
                group["type"].as_str(),
                Some("select" | "url-test" | "fallback" | "load-balance")
            ) {
                return Err("Clash 分组类型无效");
            }
            if let Some(filter) = group.get("filter")
                && !filter.as_array().is_some_and(|v| {
                    v.len() <= 32
                        && v.iter()
                            .all(|s| s.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128))
                })
            {
                return Err("筛选词应为字符串数组，星号表示全部");
            }
            if group.get("url").is_some_and(|v| {
                !v.as_str()
                    .is_some_and(|s| s.starts_with("https://") && s.len() <= 2048)
            }) {
                return Err("测速地址必须为 HTTPS");
            }
            if group
                .get("interval")
                .is_some_and(|v| !v.as_u64().is_some_and(|n| (10..=86400).contains(&n)))
            {
                return Err("测速间隔需为 10 至 86400 秒");
            }
        }
    }
    if let Some(providers) = value.get("rule-providers") {
        for (name, provider) in providers
            .as_object()
            .filter(|v| v.len() <= 64)
            .ok_or("规则提供者最多 64 项")?
        {
            if name.is_empty()
                || name.len() > 128
                || provider["type"] != "http"
                || !provider["url"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("https://") && s.len() <= 2048)
                || !matches!(
                    provider["behavior"].as_str(),
                    Some("domain" | "ipcidr" | "classical")
                )
            {
                return Err("规则提供者配置无效");
            }
            if provider.get("path").is_some() {
                return Err("规则提供者缓存路径由客户端决定");
            }
        }
    }
    Ok(())
}

pub fn apply_clash(mut profile: Value, template: &Value) -> Result<Value, &'static str> {
    validate_clash_template(template)?;
    let proxies = profile["proxies"].as_array().ok_or("缺少授权节点")?.clone();
    let names: Vec<String> = proxies
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect();
    for (key, value) in template.as_object().ok_or("模板无效")? {
        profile[key] = value.clone();
    }
    if let Some(groups) = profile["proxy-groups"].as_array_mut() {
        for group in groups {
            let mut members = group
                .get("proxies")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(filters) = group.as_object_mut().and_then(|o| o.remove("filter")) {
                for name in &names {
                    if filters
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(|f| f == "*" || name.to_lowercase().contains(&f.to_lowercase()))
                        && !members.contains(&json!(name))
                    {
                        members.push(json!(name));
                    }
                }
            }
            if group["type"] != "select" {
                if group.get("url").is_none() {
                    group["url"] = json!("https://www.gstatic.com/generate_204");
                }
                if group.get("interval").is_none() {
                    group["interval"] = json!(300);
                }
            }
            group["proxies"] = json!(members);
        }
    }
    let mut graph = proxies;
    graph.extend(
        profile["proxy-groups"]
            .as_array()
            .ok_or("Clash 分组列表无效")?
            .clone(),
    );
    graph.extend([
        json!({"name":"DIRECT"}),
        json!({"name":"REJECT"}),
        json!({"name":"REJECT-DROP"}),
    ]);
    validate_groups(&graph, "name", "proxies")?;
    let names: BTreeSet<&str> = graph.iter().filter_map(|v| v["name"].as_str()).collect();
    for rule in profile["rules"]
        .as_array()
        .filter(|v| v.len() <= 1024)
        .ok_or("Clash 规则最多 1024 条")?
    {
        let rule = rule
            .as_str()
            .filter(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
            .ok_or("Clash 规则无效")?;
        let mut fields: Vec<&str> = rule.split(',').collect();
        if fields.last() == Some(&"no-resolve") {
            fields.pop();
        }
        if !fields.last().is_some_and(|v| names.contains(v)) {
            return Err("Clash 规则引用不可用分组或节点");
        }
        if fields.first() == Some(&"RULE-SET")
            && !fields
                .get(1)
                .is_some_and(|v| profile["rule-providers"].get(*v).is_some())
        {
            return Err("Clash 规则集引用不存在");
        }
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_reject_cycles_and_unknown_authorization() {
        assert!(
            validate_groups(
                &[
                    json!({"tag":"a","outbounds":["b"]}),
                    json!({"tag":"b","outbounds":["a"]})
                ],
                "tag",
                "outbounds"
            )
            .is_err()
        );
        assert!(
            validate_groups(
                &[json!({"tag":"a","outbounds":["unauthorized"]})],
                "tag",
                "outbounds"
            )
            .is_err()
        );
    }
    #[test]
    fn region_filter_keeps_only_matching_authorized_nodes() {
        let profile = json!({"proxies":[{"name":"日本 1"},{"name":"德国 1"}],"proxy-groups":[],"rules":["MATCH,日本"]});
        let result = apply_clash(
            profile,
            &json!({"proxy-groups":[{"name":"日本","type":"url-test","filter":["日本"]}]}),
        )
        .unwrap();
        assert_eq!(result["proxy-groups"][0]["proxies"], json!(["日本 1"]));
        assert!(result["proxy-groups"][0].get("filter").is_none());
    }
}

//! Server-side extensions preserve managed identities, paths, metering and control.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerProfile {
    pub outbounds: Vec<Value>,
    pub endpoints: Vec<Value>,
    pub services: Vec<Value>,
    pub cloudflared: Vec<Value>,
    pub dns: Option<Value>,
    pub route: Option<Value>,
}

fn tag(value: &Value) -> Result<&str, &'static str> {
    value["tag"]
        .as_str()
        .filter(|s| {
            s.starts_with("extra-")
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        .ok_or("自定义对象标签必须以 extra- 开头，且只使用字母、数字、点、横线或下划线")
}
fn bounded(value: &Value, depth: usize) -> Result<(), &'static str> {
    if depth > 16 {
        return Err("配置嵌套过深");
    }
    match value {
        Value::Object(object) => {
            if value["type"] == "hosts" && object.contains_key("path") {
                return Err("DNS hosts 只允许内联记录，不能读取主机文件");
            }
            for (key, value) in object {
                if key.ends_with("_path")
                    || key.ends_with("_file")
                    || key.ends_with("_directory")
                    || [
                        "command",
                        "exec",
                        "netns",
                        "routing_mark",
                        "set_system_proxy",
                        "auto_route",
                        "auto_redirect",
                        "external_controller",
                        "external_ui",
                        "apsp_user_limits",
                        "listen_packet",
                        "certificate_directory",
                        "state_directory",
                        "data_directory",
                        "working_directory",
                        "redirect_gateway",
                        "redirect_private",
                        "csd",
                        "tncc",
                        "hip",
                    ]
                    .contains(&key.as_str())
                {
                    return Err("高级配置不能引用任意文件、命令、系统路由或控制接口");
                }
                bounded(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            if values.len() > 1024 {
                return Err("配置数组过大");
            }
            for v in values {
                bounded(v, depth + 1)?;
            }
        }
        Value::String(s) if s.len() > 65536 || s.contains('\0') => {
            return Err("配置字段过长或包含无效字符");
        }
        _ => {}
    }
    Ok(())
}

impl ServerProfile {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.outbounds.len() > 64
            || self.endpoints.len() > 16
            || self.services.len() > 16
            || self.cloudflared.len() > 8
        {
            return Err("高级对象数量超限");
        }
        let value = serde_json::to_value(self).map_err(|_| "配置无法序列化")?;
        if value.to_string().len() > 262144 {
            return Err("高级配置不能超过 256 KiB");
        }
        bounded(&value, 0)?;
        let mut tags = BTreeSet::new();
        for object in self
            .outbounds
            .iter()
            .chain(&self.endpoints)
            .chain(&self.services)
            .chain(&self.cloudflared)
        {
            if !tags.insert(tag(object)?) {
                return Err("高级对象标签重复");
            }
        }
        for outbound in &self.outbounds {
            if !matches!(
                outbound["type"].as_str(),
                Some(
                    "direct"
                        | "socks"
                        | "http"
                        | "shadowsocks"
                        | "vless"
                        | "vmess"
                        | "trojan"
                        | "hysteria2"
                        | "tuic"
                        | "anytls"
                        | "naive"
                        | "snell"
                        | "shadowtls"
                        | "selector"
                        | "urltest"
                        | "ssh"
                        | "bridge"
                )
            ) {
                return Err("出站类型不受支持");
            }
        }
        for endpoint in &self.endpoints {
            if !matches!(
                endpoint["type"].as_str(),
                Some("wireguard" | "tailscale" | "openconnect" | "openvpn-client")
            ) {
                return Err("端点类型不受支持");
            }
            if endpoint.get("system") == Some(&json!(true))
                || endpoint.get("system_interface") == Some(&json!(true))
            {
                return Err("端点必须使用用户空间网络，不创建系统接口");
            }
        }
        for service in &self.services {
            if service["type"] != "hysteria-realm" {
                return Err("仅支持 Hysteria Realm 服务");
            }
            if !service["users"].as_array().is_some_and(|v| {
                !v.is_empty()
                    && v.len() <= 128
                    && v.iter()
                        .all(|u| u["token"].as_str().is_some_and(|s| s.len() >= 32))
            }) {
                return Err("Realm 服务必须配置独立认证令牌");
            }
        }
        for tunnel in &self.cloudflared {
            if !tunnel["token"].as_str().is_some_and(|s| s.len() >= 32)
                || !tunnel["origin_node_id"].as_i64().is_some_and(|id| id > 0)
            {
                return Err("Cloudflared 需要隧道令牌和本机 VLESS 隧道源站节点");
            }
            if tunnel.as_object().is_none_or(|o| {
                o.keys().any(|k| {
                    ![
                        "tag",
                        "token",
                        "origin_node_id",
                        "protocol",
                        "ha_connections",
                        "edge_ip_version",
                    ]
                    .contains(&k.as_str())
                })
            }) {
                return Err("Cloudflared 配置包含不支持的字段");
            }
        }
        if let Some(route) = &self.route {
            if route.as_object().is_none_or(|o| {
                o.keys()
                    .any(|k| !["rules", "rule_set", "final"].contains(&k.as_str()))
            }) {
                return Err("服务端路由仅支持 rules、rule_set 和 final");
            }
            crate::profile::validate_rule_sets(route)?;
            if route
                .get("rules")
                .is_some_and(|r| !r.as_array().is_some_and(|r| r.len() <= 128))
            {
                return Err("服务端路由最多 128 条");
            }
        }
        Ok(())
    }

    pub fn apply(&self, content: &str) -> Result<String, String> {
        self.validate().map_err(str::to_owned)?;
        let mut config: Value = serde_json::from_str(content).map_err(|e| e.to_string())?;
        let mut ports: BTreeSet<u64> = config["inbounds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["listen_port"].as_u64())
            .chain([18085, 18086])
            .collect();
        for service in &self.services {
            let port = service["listen_port"]
                .as_u64()
                .filter(|p| *p > 0 && *p <= 65535)
                .ok_or("服务监听端口无效")?;
            if !ports.insert(port) {
                return Err("服务监听端口与节点或控制接口冲突".into());
            }
            if !service["listen"]
                .as_str()
                .is_some_and(|s| s.parse::<std::net::IpAddr>().is_ok())
                || service["tls"]["enabled"] != true
            {
                return Err("Realm 服务需要明确监听地址和 TLS".into());
            }
        }
        let mut existing: BTreeSet<String> = config["outbounds"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(config["endpoints"].as_array().into_iter().flatten())
            .filter_map(|v| v["tag"].as_str().map(str::to_owned))
            .collect();
        for object in self.outbounds.iter().chain(&self.endpoints) {
            if !existing.insert(tag(object).map_err(str::to_owned)?.to_owned()) {
                return Err("自定义标签与现有配置冲突".into());
            }
        }
        for outbound in self.outbounds.iter().chain(&self.endpoints) {
            for reference in outbound.get("detour").into_iter().chain(
                outbound
                    .get("outbounds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            ) {
                if !reference.as_str().is_some_and(|s| {
                    existing.contains(s) && (s.starts_with("extra-") || s == "direct")
                }) {
                    return Err("出站引用不存在或试图使用受管链路内部出站".into());
                }
            }
        }
        for (key, items) in [
            ("outbounds", &self.outbounds),
            ("endpoints", &self.endpoints),
            ("services", &self.services),
        ] {
            if items.is_empty() {
                continue;
            }
            if config.get(key).is_none() {
                config[key] = json!([]);
            }
            config[key]
                .as_array_mut()
                .ok_or("原配置列表无效")?
                .extend(items.clone());
        }
        fn check_detours(value: &Value, known: &BTreeSet<String>) -> Result<(), String> {
            match value {
                Value::Object(map) => {
                    for (key, v) in map {
                        if key == "detour"
                            && !v.as_str().is_some_and(|s| {
                                known.contains(s) && (s.starts_with("extra-") || s == "direct")
                            })
                        {
                            return Err("DNS 或端点引用内部或不存在的出口".into());
                        }
                        check_detours(v, known)?;
                    }
                }
                Value::Array(values) => {
                    for v in values {
                        check_detours(v, known)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        check_detours(
            &serde_json::to_value(self).map_err(|e| e.to_string())?,
            &existing,
        )?;
        let mut graph = config["outbounds"].as_array().cloned().unwrap_or_default();
        graph.extend(config["endpoints"].as_array().cloned().unwrap_or_default());
        for item in &mut graph {
            if let Some(detour) = item.get("detour").cloned() {
                item["outbounds"] = json!([detour]);
            }
        }
        crate::profile::validate_groups(&graph, "tag", "outbounds").map_err(str::to_owned)?;
        let public: Vec<String> = config["inbounds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["tag"].as_str())
            .filter(|s| {
                s.strip_prefix("node-")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(str::to_owned)
            .collect();
        let public: Vec<String> = public
            .into_iter()
            .map(|tag| {
                if config["inbounds"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|v| v["tag"] == tag && v["type"] == "shadowtls")
                }) {
                    format!("{tag}-inner")
                } else {
                    tag
                }
            })
            .collect();
        if config["route"].get("rules").is_none() {
            config["route"]["rules"] = json!([]);
        }
        if let Some(route) = &self.route {
            if let Some(sets) = route.get("rule_set") {
                if config["route"].get("rule_set").is_none() {
                    config["route"]["rule_set"] = json!([]);
                }
                let list = config["route"]["rule_set"]
                    .as_array_mut()
                    .ok_or("原规则集无效")?;
                for item in sets.as_array().ok_or("规则集无效")? {
                    if list.iter().any(|old| old["tag"] == item["tag"]) {
                        return Err("规则集标签冲突".into());
                    }
                    list.push(item.clone());
                }
            }
            for rule in route["rules"].as_array().into_iter().flatten() {
                if !rule.is_object() {
                    return Err("路由规则必须是对象".into());
                }
                if rule.get("outbound").is_some_and(|v| {
                    !v.as_str().is_some_and(|s| {
                        existing.contains(s) && (s.starts_with("extra-") || s == "direct")
                    })
                }) {
                    return Err("路由引用无效出站".into());
                }
                if !matches!(
                    rule.get("action").and_then(Value::as_str),
                    None | Some("route" | "reject" | "sniff" | "hijack-dns")
                ) {
                    return Err("不支持此路由动作".into());
                }
                let mut guarded = rule.clone();
                if let Some(inbounds) = rule.get("inbound") {
                    let names = if let Some(name) = inbounds.as_str() {
                        vec![name]
                    } else {
                        inbounds
                            .as_array()
                            .ok_or("入站筛选无效")?
                            .iter()
                            .map(|v| v.as_str().ok_or("入站筛选无效"))
                            .collect::<Result<Vec<_>, _>>()?
                    };
                    if names.iter().any(|name| !public.iter().any(|p| p == name)) {
                        return Err("路由只能匹配本机受管公开入站".into());
                    }
                }
                let object = guarded.as_object_mut().ok_or("路由规则无效")?;
                let action = object.remove("action").unwrap_or(json!("route"));
                let mut outer = json!({"type":"logical","mode":"and","action":action,"rules":[]});
                for key in [
                    "outbound",
                    "override_address",
                    "override_port",
                    "sniffer",
                    "timeout",
                    "method",
                    "no_drop",
                ] {
                    if let Some(value) = object.remove(key) {
                        outer[key] = value;
                    }
                }
                outer["rules"] = json!([{"inbound":public},guarded]);
                let guarded = outer;
                if !public.is_empty() {
                    config["route"]["rules"]
                        .as_array_mut()
                        .ok_or("路由规则无效")?
                        .push(guarded);
                }
            }
            if let Some(final_tag) = route.get("final") {
                if !final_tag.as_str().is_some_and(|s| {
                    existing.contains(s) && (s.starts_with("extra-") || s == "direct")
                }) {
                    return Err("默认出口无效".into());
                }
                if !public.is_empty() {
                    config["route"]["rules"]
                        .as_array_mut()
                        .ok_or("路由规则无效")?
                        .push(json!({"inbound":public,"action":"route","outbound":final_tag}));
                }
            }
        }
        if let Some(dns) = &self.dns {
            if !dns.is_object() {
                return Err("DNS 必须是对象".into());
            }
            if config.get("dns").is_some() {
                if dns.as_object().is_some_and(|o| {
                    o.keys()
                        .any(|k| !["servers", "rules", "final"].contains(&k.as_str()))
                }) {
                    return Err("链路 DNS 已存在，此处只能追加解析器和规则".into());
                }
                let current = config["dns"].as_object_mut().ok_or("原 DNS 配置无效")?;
                for key in ["servers", "rules"] {
                    if let Some(extra) = dns.get(key) {
                        let extra = extra.as_array().ok_or("DNS 列表无效")?;
                        let list = current
                            .entry(key)
                            .or_insert(json!([]))
                            .as_array_mut()
                            .ok_or("原 DNS 列表无效")?;
                        for item in extra {
                            if key == "servers" && list.iter().any(|old| old["tag"] == item["tag"])
                            {
                                return Err("DNS 标签与链路专用解析器冲突".into());
                            }
                            list.push(item.clone());
                        }
                    }
                }
                if dns.get("final").is_some() && current.get("final") != dns.get("final") {
                    return Err(
                        "此服务器已有链路 DNS，不能覆盖其默认解析器；可增加独立规则和解析器".into(),
                    );
                }
            } else {
                config["dns"] = dns.clone();
            }
        }
        for tunnel in &self.cloudflared {
            let origin = format!(
                "node-{}",
                tunnel["origin_node_id"].as_i64().ok_or("源站无效")?
            );
            let inbound = config["inbounds"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|v| v["tag"] == origin)
                .ok_or("Cloudflared 源站未启用或尚无授权")?;
            if inbound["type"] != "vless"
                || inbound["tls"]["enabled"] == true
                || !inbound["listen"]
                    .as_str()
                    .is_some_and(|s| s.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback()))
            {
                return Err("Cloudflared 源站必须是本机回环 VLESS 隧道节点".into());
            }
            let address = inbound["listen"].clone();
            let port = inbound["listen_port"].clone();
            let mut input = tunnel.clone();
            input
                .as_object_mut()
                .ok_or("隧道无效")?
                .remove("origin_node_id");
            input["type"] = json!("cloudflared");
            let output_tag = format!("{}-origin", tag(tunnel).map_err(str::to_owned)?);
            if !existing.insert(output_tag.clone()) {
                return Err("隧道源站出口标签与现有出口冲突".into());
            }
            config["outbounds"]
                .as_array_mut()
                .ok_or("出站列表无效")?
                .push(json!({"type":"direct","tag":output_tag}));
            config["inbounds"]
                .as_array_mut()
                .ok_or("入站列表无效")?
                .push(input);
            config["route"]["rules"].as_array_mut().ok_or("路由规则无效")?.insert(0,json!({"inbound":[tunnel["tag"]],"action":"route","outbound":output_tag,"override_address":address,"override_port":port}));
        }
        serde_json::to_string_pretty(&config).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inverted_rules_stay_scoped_and_internal_detours_are_rejected() {
        let content=json!({"inbounds":[{"tag":"node-1","listen_port":22001},{"tag":"internal-hop"}],"outbounds":[{"type":"direct","tag":"direct"},{"type":"direct","tag":"internal-exit"}],"route":{"final":"internal-exit"}}).to_string();
        let profile:ServerProfile=serde_json::from_value(json!({"route":{"rules":[{"invert":true,"domain_suffix":["example.com"],"outbound":"direct"}]}})).unwrap();
        let result: Value = serde_json::from_str(&profile.apply(&content).unwrap()).unwrap();
        let rule = &result["route"]["rules"][0];
        assert_eq!(rule["mode"], "and");
        assert_eq!(rule["rules"][0]["inbound"], json!(["node-1"]));
        assert_eq!(rule["rules"][1]["invert"], true);
        assert_eq!(result["route"]["final"], "internal-exit");
        let invalid:ServerProfile=serde_json::from_value(json!({"dns":{"servers":[{"tag":"extra-dns","type":"https","server":"dns.example.com","detour":"internal-exit"}]}})).unwrap();
        assert!(invalid.apply(&content).is_err());
        for port in [22001, 18085, 18086] {
            let conflict:ServerProfile=serde_json::from_value(json!({"services":[{"type":"hysteria-realm","tag":"extra-realm","listen":"127.0.0.1","listen_port":port,"tls":{"enabled":true},"users":[{"token":"TEST_ONLY-000000000000000000000000000"}]}]})).unwrap();
            assert!(conflict.apply(&content).is_err());
        }
    }
    #[test]
    fn extensions_cannot_replace_identity_or_control_and_routes_are_scoped() {
        assert!(serde_json::from_value::<ServerProfile>(json!({"experimental":{}})).is_err());
        for value in [
            json!({"dns":{"servers":[{"type":"hosts","tag":"extra-hosts","path":"/etc/hosts"}]}}),
            json!({"endpoints":[{"type":"tailscale","tag":"extra-vpn","taildrop_directory":"/tmp"}]}),
        ] {
            assert!(
                serde_json::from_value::<ServerProfile>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }

        let definition:ServerProfile=serde_json::from_value(json!({"outbounds":[{"type":"direct","tag":"extra-exit"}],"route":{"final":"extra-exit"}})).unwrap();
        let result:Value=serde_json::from_str(&definition.apply(&json!({"inbounds":[{"tag":"node-1"},{"tag":"internal-hop"}],"outbounds":[{"type":"direct","tag":"direct"}],"route":{"final":"direct","apsp_user_limits":[{}]}}).to_string()).unwrap()).unwrap();
        assert_eq!(result["route"]["rules"][0]["inbound"], json!(["node-1"]));
        assert_eq!(result["route"]["apsp_user_limits"], json!([{}]));
    }
}

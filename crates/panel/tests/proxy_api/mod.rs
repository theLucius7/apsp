//! Console paths of the proxy network, for tests that name routes from their owner's API
//! root: `/nodes/1` is `/api/traffic/nodes/1`, `/users` is `/api/access/users`.

const TRAFFIC: [&str; 11] = [
    "chains",
    "nexus-nodes",
    "node-catalog",
    "nodes",
    "ordered-proxy-resources",
    "ordered-subscription-source-jobs",
    "ordered-subscription-source-previews",
    "ordered-subscription-sources",
    "runtime-inventory",
    "runtime-rollouts",
    "servers",
];

pub fn proxy_api(path: &str) -> String {
    let segment = path
        .trim_start_matches('/')
        .split(['/', '?'])
        .next()
        .unwrap_or_default();
    let owner = if TRAFFIC.contains(&segment) {
        "traffic"
    } else {
        "access"
    };
    format!("/api/{owner}{path}")
}

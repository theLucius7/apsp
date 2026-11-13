#![forbid(unsafe_code)]
//! The request guard's verdict for every registered route, compared with the reviewed
//! table in `fixtures/route-policy.tsv`. A rule change shows up here as a difference per
//! route; `tools/route-policy.py` keeps the table's list of routes in step with the source.
//!
//! To accept intended changes: `APSP_BLESS_ROUTE_POLICY=1 cargo test -p apsp-panel --test
//! route_policy`, then review the table's diff.

use apsp_governance::control_center::guard;
use axum::http::Method;

const TABLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/route-policy.tsv"
);

/// A request path for a route: every parameter becomes the same positive number, so a
/// `servers/{id}` segment reads as a server scope like a real request's.
fn sample(route: &str) -> String {
    route
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "7"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn every_registered_route_has_its_reviewed_verdict() {
    apsp_panel::domains::install();
    let table = std::fs::read_to_string(TABLE).expect("route policy table");
    let mut blessed = String::new();
    let mut differences = Vec::new();
    let mut routes = 0;
    for line in table.lines() {
        if line.is_empty() || line.starts_with('#') {
            blessed.push_str(line);
            blessed.push('\n');
            continue;
        }
        let mut columns = line.split('\t');
        let (method, route, recorded) = (
            columns.next().expect("method"),
            columns.next().expect("route"),
            columns.next().expect("verdict"),
        );
        let verdict = guard::describe(
            &sample(route),
            &Method::from_bytes(method.as_bytes()).expect("HTTP method"),
        );
        if verdict != recorded {
            differences.push(format!(
                "{method} {route}: recorded `{recorded}`, guard `{verdict}`"
            ));
        }
        blessed.push_str(&format!("{method}\t{route}\t{verdict}\n"));
        routes += 1;
    }
    assert!(routes > 400, "the table lists the panel's routes");
    if std::env::var_os("APSP_BLESS_ROUTE_POLICY").is_some() {
        std::fs::write(TABLE, blessed).expect("write route policy table");
        return;
    }
    assert!(
        differences.is_empty(),
        "route policy differs from the reviewed table:\n{}",
        differences.join("\n")
    );
}

#[test]
fn requests_outside_the_registered_routes_stay_closed() {
    apsp_panel::domains::install();
    // An unknown management path names no capability, so only an owner session passes.
    assert_eq!(
        guard::describe("/api/unknown/thing", &Method::POST),
        "owner session only"
    );
    assert_eq!(
        guard::describe("/sub/token", &Method::GET),
        "outside the guard"
    );
}

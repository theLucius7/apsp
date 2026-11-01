//! The measurement steps for one address and port.

use super::{
    budget::{
        BASE_TESTS, CHINA_TESTS, CONTROL_PORTS, CONTROL_SEARCH_PROBES, FOLLOW_UP_TESTS, Ledger,
        OVERSEAS_TESTS,
    },
    classify::{Carrier, Control, Evidence, Measured, Probe, needs_control},
    globalping::{
        Client, MeasureError, ProbeResult, Transport, china_locations, overseas_locations,
    },
};
use serde_json::{Value, json};
use std::net::IpAddr;

pub enum Outcome {
    Measured(Evidence),
    /// Nothing was learned because the budget ran out; retried on a later run.
    Deferred,
}

/// Magic location names can match a carrier's offices outside the mainland, such
/// as China Mobile International in Hong Kong. Those probes do not cross the
/// border being measured, so they are dropped rather than counted.
fn china_probes(results: Vec<ProbeResult>) -> Vec<Probe> {
    results
        .into_iter()
        .filter(|result| {
            result
                .country
                .as_deref()
                .is_none_or(|country| country.eq_ignore_ascii_case("CN"))
        })
        .map(|result| Probe {
            carrier: Carrier::from_network(&result.network),
            connected: result.received > 0.0,
            network: result.network,
            city: result.city,
            avg_ms: result.avg_ms,
        })
        .collect()
}

fn overseas_probes(results: Vec<ProbeResult>) -> Vec<Probe> {
    results
        .into_iter()
        .map(|result| Probe {
            carrier: Carrier::Other,
            connected: result.received > 0.0,
            network: result.network,
            city: result.city,
            avg_ms: result.avg_ms,
        })
        .collect()
}

fn measured(
    result: Result<Vec<ProbeResult>, MeasureError>,
    convert: fn(Vec<ProbeResult>) -> Vec<Probe>,
) -> Measured {
    result.map(convert).map_err(MeasureError::code)
}

/// Finds a port open from overseas, then tests that same port from mainland
/// China. The node port is never a candidate: comparing it with itself would
/// turn every port block into an apparent IP block.
async fn compare<T: Transport>(
    client: &Client<'_, T>,
    ledger: &Ledger,
    ip: IpAddr,
    node_port: u16,
) -> Control {
    if !ledger.try_spend(FOLLOW_UP_TESTS) {
        return Control::Deferred;
    }
    let mut reserved = FOLLOW_UP_TESTS;
    let mut failure = None;
    let mut open = None;
    for port in CONTROL_PORTS.into_iter().filter(|port| *port != node_port) {
        reserved -= CONTROL_SEARCH_PROBES;
        match client
            .tcp_ping(ip, port, overseas_locations(CONTROL_SEARCH_PROBES))
            .await
        {
            Ok(results) if results.iter().any(|result| result.received > 0.0) => {
                open = Some(port);
                break;
            }
            Ok(_) => {}
            Err(MeasureError::RateLimited) => {
                ledger.exhaust();
                ledger.refund(reserved);
                return Control::Deferred;
            }
            Err(error) => failure = Some(error.code()),
        }
    }
    let Some(port) = open else {
        ledger.refund(reserved);
        return failure.map_or(Control::NoPort, Control::Failed);
    };
    // Any reservation beyond the mainland probes is returned.
    ledger.refund(reserved - CHINA_TESTS);
    match client.tcp_ping(ip, port, china_locations()).await {
        Ok(results) => Control::Checked {
            port,
            china: china_probes(results),
        },
        Err(MeasureError::RateLimited) => {
            ledger.exhaust();
            ledger.refund(CHINA_TESTS);
            Control::Deferred
        }
        Err(error) => Control::Failed(error.code()),
    }
}

pub async fn measure<T: Transport>(
    client: &Client<'_, T>,
    ledger: &Ledger,
    ip: IpAddr,
    port: u16,
) -> Outcome {
    if !ledger.try_spend(BASE_TESTS) {
        return Outcome::Deferred;
    }
    let (china, overseas) = tokio::join!(
        client.tcp_ping(ip, port, china_locations()),
        client.tcp_ping(ip, port, overseas_locations(OVERSEAS_TESTS)),
    );
    if [&china, &overseas]
        .iter()
        .any(|result| matches!(result, Err(MeasureError::RateLimited)))
    {
        ledger.exhaust();
        return Outcome::Deferred;
    }
    let china = measured(china, china_probes);
    let overseas = measured(overseas, overseas_probes);
    let control = if needs_control(&china, &overseas) {
        compare(client, ledger, ip, port).await
    } else {
        Control::NotNeeded
    };
    Outcome::Measured(Evidence {
        china,
        overseas,
        control,
    })
}

fn group(measured: &Measured) -> Value {
    match measured {
        Ok(probes) => json!({"probes":probes,"error":null}),
        Err(code) => json!({"probes":[],"error":code}),
    }
}

/// The carrier breakdown kept with a state and shown to administrators.
pub fn evidence_detail(evidence: &Evidence) -> Value {
    let control = match &evidence.control {
        Control::NotNeeded => json!({"status":"not_needed"}),
        Control::NoPort => json!({"status":"no_port"}),
        Control::Deferred => json!({"status":"deferred"}),
        Control::Failed(code) => json!({"status":"failed","error":code}),
        Control::Checked { port, china } => {
            json!({"status":"checked","port":port,"probes":china})
        }
    };
    json!({"china":group(&evidence.china),"overseas":group(&evidence.overseas),"control":control})
}

#[cfg(test)]
mod tests {
    use super::super::classify::{ReachabilityState, classify};
    use super::super::globalping::fake::{Scripted, probe};
    use super::*;
    use serde_json::json;

    const IP: &str = "198.51.100.7";

    fn ip() -> IpAddr {
        IP.parse().unwrap()
    }
    fn finished(results: Vec<serde_json::Value>) -> serde_json::Value {
        json!({"status":"finished","results":results})
    }
    fn china_all(rcv: u32) -> Vec<serde_json::Value> {
        vec![
            probe("China Telecom", "CN", rcv, None),
            probe("China Telecom", "CN", rcv, None),
            probe("China Unicom", "CN", rcv, None),
            probe("China Mobile", "CN", rcv, None),
        ]
    }
    fn europe(rcv: u32, limit: u64) -> Vec<serde_json::Value> {
        (0..limit)
            .map(|_| probe("Example Net", "DE", rcv, None))
            .collect()
    }
    /// China and overseas run concurrently, so replies are served by request
    /// location instead of arrival order.
    #[derive(Default)]
    struct ByLocation {
        requests: std::sync::Mutex<Vec<(String, u16)>>,
        rules: Vec<(&'static str, u16, u32)>,
    }
    impl ByLocation {
        fn new(rules: Vec<(&'static str, u16, u32)>) -> Self {
            Self {
                requests: Default::default(),
                rules,
            }
        }
        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
        fn ports(&self, region: &str) -> Vec<u16> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(location, _)| location == region)
                .map(|(_, port)| *port)
                .collect()
        }
    }
    impl Transport for ByLocation {
        async fn send(
            &self,
            request: super::super::globalping::Request,
        ) -> Result<super::super::globalping::Response, super::super::globalping::TransportFailure>
        {
            use super::super::globalping::{Method, Response};
            if request.method == Method::Post {
                let body = request.body.unwrap();
                let region = if body["locations"][0]["magic"] == "Europe" {
                    "europe"
                } else {
                    "china"
                };
                let port = body["measurementOptions"]["port"].as_u64().unwrap() as u16;
                self.requests.lock().unwrap().push((region.into(), port));
                let limit = body["locations"][0]["limit"].as_u64().unwrap();
                let id = format!("{region}-{port}-{limit}");
                return Ok(Response {
                    status: 202,
                    body: serde_json::to_vec(&json!({"id":id})).unwrap(),
                });
            }
            let id = request.path.rsplit('/').next().unwrap();
            let mut parts = id.split('-');
            let region = parts.next().unwrap();
            let port: u16 = parts.next().unwrap().parse().unwrap();
            let limit: u64 = parts.next().unwrap().parse().unwrap();
            let rcv = self
                .rules
                .iter()
                .find(|(location, rule_port, _)| *location == region && *rule_port == port)
                .map_or(0, |(_, _, rcv)| *rcv);
            let results = if region == "china" {
                china_all(rcv)
            } else {
                europe(rcv, limit)
            };
            Ok(Response {
                status: 200,
                body: serde_json::to_vec(&finished(results)).unwrap(),
            })
        }
    }

    async fn run(
        transport: &ByLocation,
        ledger: &Ledger,
        port: u16,
    ) -> (ReachabilityState, Evidence) {
        let client = Client::new(transport, None).fast_polling();
        match measure(&client, ledger, ip(), port).await {
            Outcome::Measured(evidence) => (classify(&evidence), evidence),
            Outcome::Deferred => panic!("unexpected deferral"),
        }
    }

    #[tokio::test]
    async fn a_reachable_node_uses_only_the_two_base_measurements() {
        let transport = ByLocation::new(vec![("china", 8443, 3), ("europe", 8443, 3)]);
        let ledger = Ledger::new(100);
        let (state, evidence) = run(&transport, &ledger, 8443).await;
        assert_eq!(state, ReachabilityState::Reachable);
        assert_eq!(evidence.control, Control::NotNeeded);
        assert_eq!(transport.count(), 2);
        assert_eq!(ledger.spent(), BASE_TESTS);
    }

    #[tokio::test]
    async fn control_search_stops_at_the_first_open_port_then_tests_it_from_china() {
        let transport = ByLocation::new(vec![
            ("europe", 8443, 3),
            ("europe", 443, 3),
            ("china", 443, 3),
        ]);
        let ledger = Ledger::new(100);
        let (state, evidence) = run(&transport, &ledger, 8443).await;
        assert_eq!(state, ReachabilityState::PortBlocked);
        assert!(matches!(
            evidence.control,
            Control::Checked { port: 443, .. }
        ));
        assert_eq!(transport.ports("europe"), vec![8443, 22, 443]);
        assert_eq!(transport.ports("china"), vec![8443, 443]);
        // Base 6, two single-probe searches, four mainland control probes.
        assert_eq!(ledger.spent(), BASE_TESTS + 2 + CHINA_TESTS);
    }

    #[tokio::test]
    async fn the_node_port_is_never_its_own_control() {
        let transport = ByLocation::new(vec![("europe", 443, 3), ("europe", 80, 3)]);
        let ledger = Ledger::new(100);
        let (state, evidence) = run(&transport, &ledger, 443).await;
        assert_eq!(state, ReachabilityState::IpBlocked);
        assert!(matches!(
            evidence.control,
            Control::Checked { port: 80, .. }
        ));
        assert_eq!(transport.ports("europe"), vec![443, 22, 80]);
        assert_eq!(transport.ports("china"), vec![443, 80]);
    }

    #[tokio::test]
    async fn closed_control_ports_leave_the_cause_unconfirmed_and_refund_the_reserve() {
        let transport = ByLocation::new(vec![("europe", 8443, 3)]);
        let ledger = Ledger::new(100);
        let (state, evidence) = run(&transport, &ledger, 8443).await;
        assert_eq!(state, ReachabilityState::Unconfirmed);
        assert_eq!(evidence.control, Control::NoPort);
        assert_eq!(transport.ports("china"), vec![8443]);
        assert_eq!(ledger.spent(), BASE_TESTS + 3);
    }

    #[tokio::test]
    async fn overseas_failure_skips_the_comparison_entirely() {
        let transport = ByLocation::new(vec![]);
        let ledger = Ledger::new(100);
        let (state, evidence) = run(&transport, &ledger, 8443).await;
        assert_eq!(state, ReachabilityState::ServiceDown);
        assert_eq!(evidence.control, Control::NotNeeded);
        assert_eq!(transport.count(), 2);
    }

    #[tokio::test]
    async fn an_empty_budget_defers_without_sending_anything() {
        let transport = ByLocation::new(vec![]);
        let ledger = Ledger::new(BASE_TESTS - 1);
        let client = Client::new(&transport, None).fast_polling();
        assert!(matches!(
            measure(&client, &ledger, ip(), 443).await,
            Outcome::Deferred
        ));
        assert_eq!(transport.count(), 0);
    }

    #[tokio::test]
    async fn no_budget_for_the_comparison_keeps_the_base_result() {
        let transport = ByLocation::new(vec![("europe", 8443, 3)]);
        let ledger = Ledger::new(BASE_TESTS);
        let (state, evidence) = run(&transport, &ledger, 8443).await;
        assert_eq!(state, ReachabilityState::Unconfirmed);
        assert_eq!(evidence.control, Control::Deferred);
        assert_eq!(transport.count(), 2);
    }

    #[tokio::test]
    async fn a_rate_limit_response_defers_and_stops_the_run() {
        let transport = Scripted::default()
            .reply(429, json!({}))
            .reply(429, json!({}));
        let ledger = Ledger::new(100);
        let client = Client::new(&transport, None).fast_polling();
        assert!(matches!(
            measure(&client, &ledger, ip(), 443).await,
            Outcome::Deferred
        ));
        assert!(ledger.is_exhausted());
        assert!(!ledger.try_spend(1));
    }

    #[test]
    fn probes_outside_the_mainland_do_not_count_as_china() {
        let results = vec![
            ProbeResult {
                network: "China Mobile International".into(),
                city: "Hong Kong".into(),
                country: Some("HK".into()),
                received: 3.0,
                avg_ms: Some(5.0),
            },
            ProbeResult {
                network: "China Telecom".into(),
                city: "Shanghai".into(),
                country: Some("CN".into()),
                received: 0.0,
                avg_ms: None,
            },
            ProbeResult {
                network: "China Unicom".into(),
                city: "Beijing".into(),
                country: None,
                received: 3.0,
                avg_ms: Some(40.0),
            },
        ];
        let probes = china_probes(results);
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].carrier, Carrier::Telecom);
        assert!(!probes[0].connected);
        assert_eq!(probes[1].carrier, Carrier::Unicom);
        assert!(probes[1].connected);
    }

    #[test]
    fn detail_records_the_breakdown_and_failure_codes_without_the_target() {
        let evidence = Evidence {
            china: Ok(vec![Probe {
                carrier: Carrier::Mobile,
                network: "China Mobile".into(),
                city: "Fixture".into(),
                connected: false,
                avg_ms: None,
            }]),
            overseas: Err("timeout".into()),
            control: Control::NoPort,
        };
        let detail = evidence_detail(&evidence);
        assert_eq!(detail["china"]["probes"][0]["carrier"], "mobile");
        assert_eq!(detail["overseas"]["error"], "timeout");
        assert_eq!(detail["control"]["status"], "no_port");
        assert!(!detail.to_string().contains(IP));
    }
}

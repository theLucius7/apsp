//! Pure classification of one measurement, and the debounce across runs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fewer returned probes than this cannot support a verdict about mainland China.
const MIN_CHINA_PROBES: usize = 2;
/// Both overseas probes must report before calling a service down.
const MIN_OVERSEAS_PROBES: usize = 2;
/// Consecutive failing runs needed before a target enters a blocked state.
const BLOCK_CONFIRMATIONS: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReachabilityState {
    /// Most mainland probes connect and no carrier is entirely cut off.
    Reachable,
    /// Some carriers or probes connect and others do not.
    Partial,
    /// Mainland and overseas probes all fail on the node port.
    ServiceDown,
    /// Overseas connects; mainland fails on the node port but reaches a control port.
    PortBlocked,
    /// Overseas connects; mainland fails on the node port and on the control port.
    IpBlocked,
    /// Overseas connects and mainland fails, with no control port to compare.
    Unconfirmed,
    /// Too few probes returned, or a measurement failed.
    Unknown,
    /// No node on the endpoint accepts TCP, so a TCP measurement says nothing.
    NotApplicable,
}

impl ReachabilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::Partial => "partial",
            Self::ServiceDown => "service_down",
            Self::PortBlocked => "port_blocked",
            Self::IpBlocked => "ip_blocked",
            Self::Unconfirmed => "unconfirmed",
            Self::Unknown => "unknown",
            Self::NotApplicable => "not_applicable",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "reachable" => Self::Reachable,
            "partial" => Self::Partial,
            "service_down" => Self::ServiceDown,
            "port_blocked" => Self::PortBlocked,
            "ip_blocked" => Self::IpBlocked,
            "unconfirmed" => Self::Unconfirmed,
            "unknown" => Self::Unknown,
            "not_applicable" => Self::NotApplicable,
            _ => return None,
        })
    }

    /// Unreachable from mainland China while overseas still connects. An
    /// unconfirmed target counts: the outage is real, only its cause is open.
    pub fn is_blocked(self) -> bool {
        matches!(
            self,
            Self::PortBlocked | Self::IpBlocked | Self::Unconfirmed
        )
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Reachable => "可达",
            Self::Partial => "部分运营商可达",
            Self::ServiceDown => "服务不可用（海外也无法连接）",
            Self::PortBlocked => "端口被封锁",
            Self::IpBlocked => "IP 被封锁",
            Self::Unconfirmed => "中国不可达，无法区分端口或 IP",
            Self::Unknown => "未知",
            Self::NotApplicable => "不适用 TCP 检测",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Carrier {
    Telecom,
    Unicom,
    Mobile,
    Other,
}

impl Carrier {
    pub fn from_network(network: &str) -> Self {
        let network = network.to_ascii_lowercase();
        if ["telecom", "chinanet", "ctcc"]
            .iter()
            .any(|name| network.contains(name))
        {
            Self::Telecom
        } else if ["unicom", "china169", "cucc"]
            .iter()
            .any(|name| network.contains(name))
        {
            Self::Unicom
        } else if ["mobile", "cmnet", "cmcc", "tietong"]
            .iter()
            .any(|name| network.contains(name))
        {
            Self::Mobile
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub carrier: Carrier,
    pub network: String,
    pub city: String,
    pub connected: bool,
    pub avg_ms: Option<f64>,
}

/// Probes that reported, or the stable code of the measurement failure.
pub type Measured = Result<Vec<Probe>, String>;

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// The node port connected or overseas failed, so no comparison was needed.
    NotNeeded,
    /// No candidate port is open from overseas, so a closed port cannot be told apart.
    NoPort,
    /// The test budget ran out before the comparison could be made.
    Deferred,
    /// A measurement failed before the comparison finished.
    Failed(String),
    /// The candidate was open from overseas and was then tested from mainland China.
    Checked { port: u16, china: Vec<Probe> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub china: Measured,
    pub overseas: Measured,
    pub control: Control,
}

fn connected(probes: &[Probe]) -> usize {
    probes.iter().filter(|probe| probe.connected).count()
}

/// Whether the node port failed from mainland China while still connecting
/// overseas, the only case in which a control port can add information.
pub fn needs_control(china: &Measured, overseas: &Measured) -> bool {
    matches!(china, Ok(probes) if probes.len() >= MIN_CHINA_PROBES && connected(probes) == 0)
        && matches!(overseas, Ok(probes) if connected(probes) > 0)
}

pub fn classify(evidence: &Evidence) -> ReachabilityState {
    use ReachabilityState::*;
    let Ok(china) = &evidence.china else {
        return Unknown;
    };
    if china.len() < MIN_CHINA_PROBES {
        return Unknown;
    }
    let reached = connected(china);
    if reached > 0 {
        let mut carriers: BTreeMap<Carrier, bool> = BTreeMap::new();
        for probe in china {
            *carriers.entry(probe.carrier).or_default() |= probe.connected;
        }
        let carrier_out = carriers.values().any(|reached| !reached);
        return if !carrier_out && reached * 2 > china.len() {
            Reachable
        } else {
            Partial
        };
    }
    let Ok(overseas) = &evidence.overseas else {
        return Unknown;
    };
    if connected(overseas) == 0 {
        return if overseas.len() < MIN_OVERSEAS_PROBES {
            Unknown
        } else {
            ServiceDown
        };
    }
    match &evidence.control {
        Control::NotNeeded | Control::Failed(_) => Unknown,
        Control::NoPort | Control::Deferred => Unconfirmed,
        // One connection proves the address is routable, however few probes reported.
        Control::Checked { china, .. } if connected(china) > 0 => PortBlocked,
        Control::Checked { china, .. } if china.len() < MIN_CHINA_PROBES => Unknown,
        Control::Checked { .. } => IpBlocked,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tracked {
    pub state: ReachabilityState,
    pub pending: Option<ReachabilityState>,
    pub failing_streak: u32,
}

impl Default for Tracked {
    fn default() -> Self {
        Self {
            state: ReachabilityState::Unknown,
            pending: None,
            failing_streak: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    None,
    Blocked {
        from: ReachabilityState,
        to: ReachabilityState,
    },
    Cleared {
        from: ReachabilityState,
        to: ReachabilityState,
    },
}

/// Applies one run's observation. Entering a blocked state takes consecutive
/// failing runs; leaving takes one positive observation, and a run that learned
/// nothing neither counts toward a block nor clears one.
pub fn advance(previous: Tracked, observed: ReachabilityState) -> (Tracked, Transition) {
    use ReachabilityState::Unknown;
    if observed.is_blocked() {
        let failing_streak = previous.failing_streak.saturating_add(1);
        if previous.state.is_blocked() {
            let next = Tracked {
                state: observed,
                pending: None,
                failing_streak,
            };
            return (next, Transition::None);
        }
        if failing_streak < BLOCK_CONFIRMATIONS {
            let next = Tracked {
                state: previous.state,
                pending: Some(observed),
                failing_streak,
            };
            return (next, Transition::None);
        }
        let next = Tracked {
            state: observed,
            pending: None,
            failing_streak,
        };
        let transition = Transition::Blocked {
            from: previous.state,
            to: observed,
        };
        return (next, transition);
    }
    if observed == Unknown {
        let state = if previous.state.is_blocked() {
            previous.state
        } else {
            Unknown
        };
        return (Tracked { state, ..previous }, Transition::None);
    }
    let next = Tracked {
        state: observed,
        pending: None,
        failing_streak: 0,
    };
    let transition = if previous.state.is_blocked() {
        Transition::Cleared {
            from: previous.state,
            to: observed,
        }
    } else {
        Transition::None
    };
    (next, transition)
}

#[cfg(test)]
mod tests {
    use super::ReachabilityState::*;
    use super::*;

    fn probe(carrier: Carrier, connected: bool) -> Probe {
        Probe {
            carrier,
            network: String::new(),
            city: String::new(),
            connected,
            avg_ms: connected.then_some(30.0),
        }
    }
    fn china(telecom: &[bool], unicom: &[bool], mobile: &[bool]) -> Vec<Probe> {
        telecom
            .iter()
            .map(|c| probe(Carrier::Telecom, *c))
            .chain(unicom.iter().map(|c| probe(Carrier::Unicom, *c)))
            .chain(mobile.iter().map(|c| probe(Carrier::Mobile, *c)))
            .collect()
    }
    fn overseas(results: &[bool]) -> Vec<Probe> {
        results.iter().map(|c| probe(Carrier::Other, *c)).collect()
    }
    fn evidence(china: Vec<Probe>, overseas: Vec<Probe>, control: Control) -> Evidence {
        Evidence {
            china: Ok(china),
            overseas: Ok(overseas),
            control,
        }
    }
    fn all_failed() -> Vec<Probe> {
        china(&[false, false], &[false], &[false])
    }

    #[test]
    fn every_carrier_connecting_is_reachable() {
        let all = china(&[true, true], &[true], &[true]);
        assert_eq!(
            classify(&evidence(all, overseas(&[true, true]), Control::NotNeeded)),
            Reachable
        );
        // One Telecom probe failing while the carrier still connects is not partial.
        let most = china(&[true, false], &[true], &[true]);
        assert_eq!(
            classify(&evidence(most, overseas(&[true, true]), Control::NotNeeded)),
            Reachable
        );
    }

    #[test]
    fn a_whole_carrier_failing_is_partial_even_when_most_probes_connect() {
        let mobile_out = china(&[true, true], &[true], &[false]);
        assert_eq!(
            classify(&evidence(
                mobile_out,
                overseas(&[true, true]),
                Control::NotNeeded
            )),
            Partial
        );
        let telecom_out = china(&[false, false], &[true], &[true]);
        assert_eq!(
            classify(&evidence(
                telecom_out,
                overseas(&[true, true]),
                Control::NotNeeded
            )),
            Partial
        );
    }

    #[test]
    fn half_of_one_carrier_is_partial_not_reachable() {
        let half = china(&[true, false], &[], &[]);
        assert_eq!(
            classify(&evidence(half, overseas(&[true, true]), Control::NotNeeded)),
            Partial
        );
    }

    #[test]
    fn a_mainland_connection_outranks_an_overseas_failure() {
        let all = china(&[true, true], &[true], &[true]);
        assert_eq!(
            classify(&evidence(
                all,
                overseas(&[false, false]),
                Control::NotNeeded
            )),
            Reachable
        );
    }

    #[test]
    fn overseas_failure_is_service_down_never_a_block() {
        assert_eq!(
            classify(&evidence(
                all_failed(),
                overseas(&[false, false]),
                Control::NotNeeded
            )),
            ServiceDown
        );
    }

    #[test]
    fn one_overseas_connection_is_enough_to_show_the_service_is_up() {
        assert_eq!(
            classify(&evidence(
                all_failed(),
                overseas(&[true, false]),
                Control::NoPort
            )),
            Unconfirmed
        );
    }

    #[test]
    fn control_port_tells_port_blocks_from_ip_blocks() {
        let port = evidence(
            all_failed(),
            overseas(&[true, true]),
            Control::Checked {
                port: 22,
                china: china(&[true, false], &[false], &[false]),
            },
        );
        assert_eq!(classify(&port), PortBlocked);
        let ip = evidence(
            all_failed(),
            overseas(&[true, true]),
            Control::Checked {
                port: 22,
                china: all_failed(),
            },
        );
        assert_eq!(classify(&ip), IpBlocked);
    }

    #[test]
    fn a_closed_control_port_never_reads_as_an_ip_block() {
        // Port 22 closed everywhere means no candidate was open from overseas.
        let closed = evidence(all_failed(), overseas(&[true, true]), Control::NoPort);
        assert_eq!(classify(&closed), Unconfirmed);
    }

    #[test]
    fn a_comparison_deferred_for_budget_stays_unconfirmed() {
        let deferred = evidence(all_failed(), overseas(&[true, true]), Control::Deferred);
        assert_eq!(classify(&deferred), Unconfirmed);
    }

    #[test]
    fn too_few_probes_or_errors_are_unknown() {
        let thin = china(&[false], &[], &[]);
        assert_eq!(
            classify(&evidence(thin, overseas(&[true, true]), Control::NoPort)),
            Unknown
        );
        let one_success = china(&[true], &[], &[]);
        assert_eq!(
            classify(&evidence(
                one_success,
                overseas(&[true, true]),
                Control::NotNeeded
            )),
            Unknown
        );
        assert_eq!(
            classify(&Evidence {
                china: Err("http_500".into()),
                overseas: Ok(overseas(&[true, true])),
                control: Control::NotNeeded,
            }),
            Unknown
        );
        assert_eq!(
            classify(&Evidence {
                china: Ok(all_failed()),
                overseas: Err("timeout".into()),
                control: Control::NotNeeded,
            }),
            Unknown
        );
        assert_eq!(
            classify(&evidence(all_failed(), overseas(&[false]), Control::NoPort)),
            Unknown
        );
        assert_eq!(
            classify(&evidence(
                all_failed(),
                overseas(&[true, true]),
                Control::Failed("timeout".into())
            )),
            Unknown
        );
        let thin_control = evidence(
            all_failed(),
            overseas(&[true, true]),
            Control::Checked {
                port: 443,
                china: china(&[false], &[], &[]),
            },
        );
        assert_eq!(classify(&thin_control), Unknown);
    }

    #[test]
    fn a_single_control_connection_outweighs_thin_results() {
        let thin = evidence(
            all_failed(),
            overseas(&[true, true]),
            Control::Checked {
                port: 80,
                china: china(&[true], &[], &[]),
            },
        );
        assert_eq!(classify(&thin), PortBlocked);
    }

    #[test]
    fn control_is_only_needed_when_mainland_fails_and_overseas_connects() {
        assert!(needs_control(
            &Ok(all_failed()),
            &Ok(overseas(&[true, false]))
        ));
        assert!(!needs_control(
            &Ok(china(&[true, false], &[false], &[false])),
            &Ok(overseas(&[true, true]))
        ));
        assert!(!needs_control(
            &Ok(all_failed()),
            &Ok(overseas(&[false, false]))
        ));
        assert!(!needs_control(
            &Ok(china(&[false], &[], &[])),
            &Ok(overseas(&[true, true]))
        ));
        assert!(!needs_control(
            &Err("timeout".into()),
            &Ok(overseas(&[true, true]))
        ));
    }

    #[test]
    fn carriers_are_recognised_from_probe_network_names() {
        for (network, carrier) in [
            ("China Telecom Group", Carrier::Telecom),
            ("CHINANET-BACKBONE", Carrier::Telecom),
            ("China Unicom Beijing", Carrier::Unicom),
            ("CHINA169-Backbone", Carrier::Unicom),
            ("China Mobile Communications", Carrier::Mobile),
            ("Example Hosting", Carrier::Other),
        ] {
            assert_eq!(Carrier::from_network(network), carrier);
        }
    }

    fn run(mut tracked: Tracked, observations: &[ReachabilityState]) -> Tracked {
        for observed in observations {
            tracked = advance(tracked, *observed).0;
        }
        tracked
    }
    fn tracked(state: ReachabilityState) -> Tracked {
        Tracked {
            state,
            ..Tracked::default()
        }
    }

    #[test]
    fn one_failing_run_only_marks_the_block_as_pending() {
        let (next, transition) = advance(tracked(Reachable), IpBlocked);
        assert_eq!(next.state, Reachable);
        assert_eq!(next.pending, Some(IpBlocked));
        assert_eq!(next.failing_streak, 1);
        assert_eq!(transition, Transition::None);
    }

    #[test]
    fn a_second_consecutive_failure_enters_the_block() {
        let once = advance(tracked(Reachable), IpBlocked).0;
        let (next, transition) = advance(once, IpBlocked);
        assert_eq!(next.state, IpBlocked);
        assert_eq!(next.pending, None);
        assert_eq!(
            transition,
            Transition::Blocked {
                from: Reachable,
                to: IpBlocked
            }
        );
    }

    #[test]
    fn differing_failures_still_count_and_the_latest_wins() {
        let (next, transition) = advance(advance(tracked(Reachable), PortBlocked).0, IpBlocked);
        assert_eq!(next.state, IpBlocked);
        assert!(matches!(transition, Transition::Blocked { .. }));
    }

    #[test]
    fn a_healthy_run_between_failures_restarts_the_count() {
        let tracked = run(tracked(Reachable), &[IpBlocked, Reachable, IpBlocked]);
        assert_eq!(tracked.state, Reachable);
        assert_eq!(tracked.failing_streak, 1);
    }

    #[test]
    fn a_first_ever_failure_cannot_enter_a_block_directly() {
        let next = advance(Tracked::default(), IpBlocked).0;
        assert_eq!(next.state, Unknown);
        assert_eq!(next.pending, Some(IpBlocked));
    }

    #[test]
    fn unknown_neither_counts_as_a_failure_nor_clears_one() {
        let pending = run(tracked(Reachable), &[IpBlocked, Unknown]);
        assert_eq!(pending.state, Unknown);
        assert_eq!(pending.pending, Some(IpBlocked));
        assert_eq!(pending.failing_streak, 1);
        let (confirmed, transition) = advance(pending, IpBlocked);
        assert_eq!(confirmed.state, IpBlocked);
        assert!(matches!(transition, Transition::Blocked { .. }));

        let (held, transition) = advance(confirmed, Unknown);
        assert_eq!(held.state, IpBlocked);
        assert_eq!(transition, Transition::None);
    }

    #[test]
    fn one_positive_run_leaves_a_block_and_reports_it() {
        let blocked = run(tracked(Reachable), &[IpBlocked, IpBlocked]);
        let (next, transition) = advance(blocked, Reachable);
        assert_eq!(next, tracked(Reachable));
        assert_eq!(
            transition,
            Transition::Cleared {
                from: IpBlocked,
                to: Reachable
            }
        );
        let (down, transition) = advance(blocked, ServiceDown);
        assert_eq!(down.state, ServiceDown);
        assert!(matches!(transition, Transition::Cleared { .. }));
    }

    #[test]
    fn moving_between_blocked_states_is_not_a_new_transition() {
        let blocked = run(tracked(Reachable), &[PortBlocked, PortBlocked]);
        let (next, transition) = advance(blocked, IpBlocked);
        assert_eq!(next.state, IpBlocked);
        assert_eq!(transition, Transition::None);
    }

    #[test]
    fn service_down_and_partial_take_effect_immediately() {
        for observed in [ServiceDown, Partial, NotApplicable] {
            let (next, transition) = advance(tracked(Reachable), observed);
            assert_eq!(next.state, observed);
            assert_eq!(transition, Transition::None);
        }
    }

    #[test]
    fn state_names_round_trip_and_blocked_states_are_exactly_three() {
        for state in [
            Reachable,
            Partial,
            ServiceDown,
            PortBlocked,
            IpBlocked,
            Unconfirmed,
            Unknown,
            NotApplicable,
        ] {
            assert_eq!(ReachabilityState::parse(state.as_str()), Some(state));
            assert_eq!(
                state.is_blocked(),
                matches!(state, PortBlocked | IpBlocked | Unconfirmed)
            );
        }
        assert_eq!(ReachabilityState::parse("other"), None);
    }
}

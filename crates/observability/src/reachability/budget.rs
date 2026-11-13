//! Test budget for one run. Each probe counts as one test against the limit.

use apsp_cloud_api::globalping::Limits;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// China Telecom twice, China Unicom and China Mobile once each.
pub const CHINA_TESTS: u32 = 4;
pub const OVERSEAS_TESTS: u32 = 2;
/// One probe per candidate: a false negative costs a comparison, never a verdict.
pub const CONTROL_SEARCH_PROBES: u32 = 1;
/// Compared in this order, stopping at the first one open from overseas.
pub const CONTROL_PORTS: [u16; 3] = [22, 443, 80];
pub const BASE_TESTS: u32 = CHINA_TESTS + OVERSEAS_TESTS;
pub const FOLLOW_UP_TESTS: u32 = CONTROL_PORTS.len() as u32 * CONTROL_SEARCH_PROBES + CHINA_TESTS;
/// The monitor never takes more than this share of the provider's limit.
const LIMIT_PERCENT: u64 = 80;

/// Tests this run may still use. Tests already spent in the window, by this
/// panel or anything else sharing the IP address, count against the same cap.
pub fn allowance(limits: Limits) -> u32 {
    let cap = u64::from(limits.limit) * LIMIT_PERCENT / 100;
    let used = u64::from(limits.limit.saturating_sub(limits.remaining));
    let left = cap.saturating_sub(used).min(u64::from(limits.remaining));
    u32::try_from(left).unwrap_or(u32::MAX)
}

/// How many leading targets fit. A target that failed last time is likely to
/// need its comparison again, so its follow-up is reserved up front. Admission
/// stops at the first target that does not fit: the list is oldest-first and
/// skipping ahead would starve the expensive ones.
pub fn plan(allowance: u32, failing_last_run: &[bool]) -> usize {
    let mut left = allowance;
    for (index, failing) in failing_last_run.iter().enumerate() {
        let cost = BASE_TESTS + if *failing { FOLLOW_UP_TESTS } else { 0 };
        if cost > left {
            return index;
        }
        left -= cost;
    }
    failing_last_run.len()
}

/// Shared spending record across the concurrent targets of a run.
#[derive(Debug)]
pub struct Ledger {
    left: AtomicU32,
    spent: AtomicU32,
    exhausted: AtomicBool,
}

impl Ledger {
    pub fn new(allowance: u32) -> Self {
        Self {
            left: AtomicU32::new(allowance),
            spent: AtomicU32::new(0),
            exhausted: AtomicBool::new(false),
        }
    }

    pub fn try_spend(&self, tests: u32) -> bool {
        if self.exhausted.load(Ordering::SeqCst) {
            return false;
        }
        let mut left = self.left.load(Ordering::SeqCst);
        loop {
            let Some(next) = left.checked_sub(tests) else {
                return false;
            };
            match self
                .left
                .compare_exchange_weak(left, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => left = actual,
            }
        }
        self.spent.fetch_add(tests, Ordering::SeqCst);
        true
    }

    /// Returns reserved tests that were not used.
    pub fn refund(&self, tests: u32) {
        self.left.fetch_add(tests, Ordering::SeqCst);
        self.spent.fetch_sub(tests, Ordering::SeqCst);
    }

    /// The provider refused for rate limits, whatever the local count says.
    pub fn exhaust(&self) {
        self.exhausted.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn is_exhausted(&self) -> bool {
        self.exhausted.load(Ordering::SeqCst)
    }

    pub fn spent(&self) -> u32 {
        self.spent.load(Ordering::SeqCst)
    }
}

/// When the next run should start. Budget deferral waits for the window to
/// reset, since an earlier attempt could not be admitted anyway.
pub fn next_run_at(
    now: i64,
    interval_secs: i64,
    earliest_due: Option<i64>,
    deferred_reset_secs: Option<u32>,
) -> i64 {
    const MINIMUM_GAP: i64 = 60;
    if let Some(reset) = deferred_reset_secs {
        return now + (i64::from(reset) + 5).clamp(MINIMUM_GAP, 3600);
    }
    earliest_due
        .unwrap_or(now + interval_secs)
        .clamp(now + MINIMUM_GAP, now + interval_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(limit: u32, remaining: u32) -> Limits {
        Limits {
            limit,
            remaining,
            reset_secs: 600,
        }
    }

    #[test]
    fn allowance_stays_within_eighty_percent_of_the_limit() {
        assert_eq!(allowance(limits(250, 250)), 200);
        assert_eq!(allowance(limits(250, 200)), 150);
        assert_eq!(allowance(limits(250, 50)), 0);
        assert_eq!(allowance(limits(250, 0)), 0);
        assert_eq!(allowance(limits(0, 0)), 0);
    }

    #[test]
    fn allowance_tolerates_inconsistent_provider_numbers() {
        assert_eq!(allowance(limits(250, 900)), 200);
        assert_eq!(allowance(limits(u32::MAX, u32::MAX)), 3_435_973_836);
    }

    #[test]
    fn plan_admits_a_prefix_and_defers_the_remainder() {
        assert_eq!(plan(200, &[false; 10]), 10);
        assert_eq!(plan(18, &[false; 10]), 3);
        assert_eq!(plan(17, &[false; 10]), 2);
        assert_eq!(plan(5, &[false; 10]), 0);
        assert_eq!(plan(0, &[]), 0);
    }

    #[test]
    fn plan_reserves_the_follow_up_for_targets_that_failed_last_time() {
        assert_eq!(BASE_TESTS, 6);
        assert_eq!(FOLLOW_UP_TESTS, 7);
        assert_eq!(plan(13, &[true]), 1);
        assert_eq!(plan(12, &[true]), 0);
        // The expensive target at the head blocks later ones instead of being starved.
        assert_eq!(plan(12, &[true, false, false]), 0);
        assert_eq!(plan(19, &[false, true, false]), 2);
    }

    #[test]
    fn a_run_cannot_overspend_its_allowance_even_in_the_worst_case() {
        for allowance in [0, 5, 6, 13, 50, 200] {
            let failing: Vec<bool> = (0..40).map(|index| index % 3 == 0).collect();
            let admitted = plan(allowance, &failing);
            let reserved: u32 = failing[..admitted]
                .iter()
                .map(|failing| BASE_TESTS + if *failing { FOLLOW_UP_TESTS } else { 0 })
                .sum();
            assert!(reserved <= allowance);
        }
    }

    #[test]
    fn ledger_refuses_past_the_allowance_and_refunds_unused_tests() {
        let ledger = Ledger::new(10);
        assert!(ledger.try_spend(6));
        assert!(!ledger.try_spend(7));
        assert_eq!(ledger.spent(), 6);
        assert!(ledger.try_spend(4));
        ledger.refund(3);
        assert_eq!(ledger.spent(), 7);
        assert!(ledger.try_spend(3));
        assert!(!ledger.try_spend(1));
    }

    #[test]
    fn a_provider_refusal_stops_further_spending_despite_refunds() {
        let ledger = Ledger::new(100);
        assert!(ledger.try_spend(6));
        ledger.exhaust();
        ledger.refund(6);
        assert!(ledger.is_exhausted());
        assert!(!ledger.try_spend(1));
    }

    #[test]
    fn deferral_waits_for_the_window_reset_within_bounds() {
        assert_eq!(next_run_at(1000, 1800, None, Some(600)), 1605);
        assert_eq!(next_run_at(1000, 1800, None, Some(0)), 1060);
        assert_eq!(next_run_at(1000, 1800, None, Some(90_000)), 4600);
    }

    #[test]
    fn the_next_run_follows_the_earliest_due_target_within_the_interval() {
        assert_eq!(next_run_at(1000, 1800, None, None), 2800);
        assert_eq!(next_run_at(1000, 1800, Some(1500), None), 1500);
        assert_eq!(next_run_at(1000, 1800, Some(900), None), 1060);
        assert_eq!(next_run_at(1000, 1800, Some(9000), None), 2800);
    }
}

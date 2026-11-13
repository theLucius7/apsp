//! Queued messages: every table that schedules another attempt at a message is a queue its
//! owner declares and implements for the platform's deliverer
//! (`apsp_panel_host::notifications::queue`), so claiming, retry waits and giving up are
//! written once and each queue only states its policy.

use super::super::{access, costs, observability};
use super::sources;
use apsp_panel_host::notifications::queue::{Backoff, Claiming, Policy};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Every crate that keeps message queues, by directory, with the queues it declares.
const DECLARATIONS: &[(&str, &[Policy])] = &[
    ("observability", observability::QUEUES),
    ("costs", costs::QUEUES),
    ("access", access::QUEUES),
];

/// The platform's deliverer: the only code that works out retry waits.
const DELIVERER: &str = "panel-host/src/notifications/queue.rs";

/// Tables of the database baseline with a `next_attempt_at` column: they schedule
/// another attempt at what they hold.
fn queue_tables() -> BTreeSet<String> {
    let baseline = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_current.sql");
    let sql = std::fs::read_to_string(baseline).expect("database baseline");
    let mut tables = BTreeSet::new();
    let mut table = None;
    for line in sql.lines() {
        if let Some(rest) = line.strip_prefix("CREATE TABLE public.") {
            table = rest.split_whitespace().next().map(str::to_owned);
        } else if line.starts_with(')') {
            table = None;
        } else if line.trim_start().starts_with("next_attempt_at ")
            && let Some(name) = &table
        {
            tables.insert(name.clone());
        }
    }
    assert!(!tables.is_empty(), "the baseline has message queues");
    tables
}

fn implementations(source: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim_start().starts_with("impl") && line.contains(" Queue for "))
        .count()
}

#[test]
fn every_queue_table_has_one_declared_queue() {
    let declared: Vec<&str> = DECLARATIONS
        .iter()
        .flat_map(|(_, queues)| queues.iter().map(|policy| policy.table))
        .collect();
    let tables: BTreeSet<String> = declared.iter().map(|table| (*table).to_owned()).collect();
    assert_eq!(
        tables.len(),
        declared.len(),
        "one queue per table: {declared:?}"
    );
    assert_eq!(
        tables,
        queue_tables(),
        "every table that schedules attempts is a queue its owner declares, drained by the \
         platform's deliverer"
    );
}

#[test]
fn every_queue_policy_can_be_followed() {
    for (owner, queues) in DECLARATIONS {
        for policy in *queues {
            let table = policy.table;
            assert!(policy.claims > 0, "{owner}: {table} claims something");
            assert!(policy.attempts > 0, "{owner}: {table} sends at least once");
            if let Claiming::Leased { seconds, batch } = policy.claiming {
                assert!(seconds > 0 && batch > 0, "{owner}: {table} leases messages");
            }
            match policy.backoff {
                Backoff::Doubling {
                    first, min, max, ..
                } => assert!(
                    first > 0 && min > 0 && min <= max,
                    "{owner}: {table} waits within its bounds"
                ),
                Backoff::Fixed(seconds) => assert!(seconds > 0, "{owner}: {table} waits"),
            }
            assert!(
                policy.held.is_none_or(|seconds| seconds > 0),
                "{owner}: {table} defers held messages"
            );
        }
    }
}

/// Each crate declares exactly the queues it implements, so a new queue cannot be left out
/// of the check above.
#[test]
fn every_queue_implementation_is_declared() {
    let mut implemented: BTreeMap<String, usize> = BTreeMap::new();
    for (path, source) in sources() {
        let count = implementations(&source);
        if count > 0 && path != DELIVERER {
            let owner = path.split('/').next().unwrap_or_default().to_owned();
            *implemented.entry(owner).or_default() += count;
        }
    }
    let declared: BTreeMap<String, usize> = DECLARATIONS
        .iter()
        .map(|(owner, queues)| ((*owner).to_owned(), queues.len()))
        .collect();
    assert_eq!(
        implemented, declared,
        "queues implemented and declared by crate"
    );
}

/// A statement that changes the attempt counter or the next attempt of a queued message is
/// part of a claim or a settlement, so it lives with a queue implementation.
#[test]
fn only_queue_implementations_reschedule_messages() {
    let tables = queue_tables();
    let mut found = BTreeSet::new();
    for (path, source) in sources() {
        let reschedules = tables.iter().any(|table| {
            source
                .match_indices(&format!("UPDATE {table} "))
                .any(|(at, _)| {
                    let statement = &source[at..];
                    let statement = &statement[..statement.find('"').unwrap_or(statement.len())];
                    statement.contains("next_attempt_at") || statement.contains("attempts")
                })
        });
        if reschedules && implementations(&source) == 0 {
            found.insert(path);
        }
    }
    assert!(
        found.is_empty(),
        "messages are claimed and settled through apsp_panel_host::notifications::queue: \
         {found:?}"
    );
}

/// Backoff from an attempt count is the deliverer's; a queue states it in its policy.
#[test]
fn only_the_deliverer_computes_retry_waits() {
    let found: Vec<String> = sources()
        .into_iter()
        .filter(|(path, source)| {
            path != DELIVERER
                && source.lines().any(|line| {
                    (line.contains("<<") || line.contains(".pow("))
                        && line.to_ascii_lowercase().contains("attempt")
                })
        })
        .map(|(path, _)| path)
        .collect();
    assert!(
        found.is_empty(),
        "retry waits come from the queue's policy in {DELIVERER}: {found:?}"
    );
}

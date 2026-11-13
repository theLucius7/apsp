//! Guards that keep each underlying mechanism single. Background work: every loop of the
//! panel's crates runs on `apsp_panel_host::maintenance::periodic`, and the heartbeat names
//! those loops record are exactly the ones the health view expects. Advisory locks: see
//! `locks`. Queued messages: see `queues`. The audit trail, passwords, random values, short
//! texts, dates, cookies and the panel settings: see `mechanisms`.

mod locks;
mod mechanisms;
mod queues;

use super::HEARTBEATS;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Source files that build tokio intervals for something other than a background
/// loop, with how many they build and why.
const NOT_BACKGROUND: &[(&str, usize, &str)] = &[
    (
        "governance/src/control_center/events.rs",
        1,
        "polls the audit log for one console event stream while its request is open",
    ),
    (
        "resources/src/device_channel.rs",
        1,
        "pings one connected Agent while its WebSocket is open",
    ),
    (
        "traffic/src/subscription_sources/worker.rs",
        2,
        "watches one running job for cancellation, and renews the leases of the jobs \
         a bounded poll started until they finish",
    ),
];

/// Crates without panel background work (the Agent, its adapters and probe, the
/// configuration compiler and the protocol) and the executor itself.
fn skipped(path: &str) -> bool {
    let krate = path.split('/').next().unwrap_or_default();
    matches!(
        krate,
        "agent" | "agent-core" | "compiler" | "protocol" | "tcp-probe"
    ) || krate.starts_with("adapter-")
        || path == "panel-host/src/maintenance.rs"
}

/// Every scanned source file by its path under `crates/`; test files, such as this one,
/// are left out.
fn sources() -> BTreeMap<String, String> {
    fn walk(directory: &Path, root: &Path, found: &mut BTreeMap<String, String>) {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .expect("source directory")
            .map(|entry| entry.expect("source entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let relative = path
                .strip_prefix(root)
                .expect("path under crates/")
                .to_string_lossy()
                .replace('\\', "/");
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if path.is_dir() {
                if name != "tests" {
                    walk(&path, root, found);
                }
            } else if name.ends_with(".rs")
                && name != "tests.rs"
                && !name.ends_with("_tests.rs")
                && !skipped(&relative)
            {
                let text = std::fs::read_to_string(&path).expect("source file");
                found.insert(relative, text);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory");
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(root).expect("crates directory") {
        let source = entry.expect("crate entry").path().join("src");
        if source.is_dir() {
            walk(&source, root, &mut found);
        }
    }
    assert!(found.len() > 100, "the scan found the panel's sources");
    found
}

/// The tokio intervals a source builds or imports.
fn intervals(source: &str) -> usize {
    let imported = source
        .match_indices("time::{")
        .filter(|(at, opening)| {
            let list = &source[at + opening.len()..];
            list[..list.find('}').unwrap_or(list.len())]
                .split(',')
                .any(|item| matches!(item.trim(), "interval" | "interval_at"))
        })
        .count();
    source.matches("time::interval").count() + imported
}

/// The string literal a call starts with, for every call of `function` in `source`.
fn first_arguments(source: &str, function: &str) -> Vec<String> {
    let call = format!("{function}(");
    source
        .match_indices(&call)
        .map(|(at, _)| {
            let rest = source[at + call.len()..].trim_start();
            let name = rest
                .strip_prefix('"')
                .unwrap_or_else(|| panic!("{function} takes the heartbeat name as a literal"));
            name[..name.find('"').expect("closing quote")].to_owned()
        })
        .collect()
}

#[test]
fn background_loops_run_on_the_periodic_executor() {
    let found: BTreeMap<String, usize> = sources()
        .into_iter()
        .map(|(path, text)| (path, intervals(&text)))
        .filter(|(_, count)| *count > 0)
        .collect();
    let allowed: BTreeMap<String, usize> = NOT_BACKGROUND
        .iter()
        .map(|(path, count, _)| ((*path).to_owned(), *count))
        .collect();
    assert_eq!(
        found, allowed,
        "background loops run on apsp_panel_host::maintenance::periodic; an interval \
         for anything else is listed in NOT_BACKGROUND with the reason"
    );
}

#[test]
fn every_periodic_task_reports_a_heartbeat_the_health_view_expects() {
    let sources = sources();
    let mut tasks = Vec::new();
    let mut beats = BTreeSet::new();
    for text in sources.values() {
        tasks.extend(first_arguments(text, "periodic"));
        beats.extend(first_arguments(text, "beat_while"));
    }
    let names: BTreeSet<String> = tasks.iter().cloned().collect();
    assert_eq!(
        names.len(),
        tasks.len(),
        "one task per heartbeat: {tasks:?}"
    );
    let expected: BTreeSet<String> = HEARTBEATS.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        expected.len(),
        HEARTBEATS.len(),
        "heartbeat names are listed once"
    );
    assert_eq!(
        names, expected,
        "HEARTBEATS lists the name of every periodic task"
    );
    assert!(
        beats.is_subset(&names),
        "a pass keeps its own task's heartbeat alive: {beats:?}"
    );
}

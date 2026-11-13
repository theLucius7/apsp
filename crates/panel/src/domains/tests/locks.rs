//! Advisory lock keys: every number names one purpose, every declaration is listed, and the
//! database baseline takes only declared keys (`apsp_panel_host::locks`).

use super::super::{
    access, costs, governance, naming, networking, observability, operations, resources, traffic,
};
use apsp_panel_host::locks::{self, Id, Key, TextKey};
use std::{collections::BTreeMap, fs, path::Path};

/// Every crate that declares advisory lock keys, by directory, with its keys and its text
/// keys (`apsp_panel_host::locks`).
const DECLARATIONS: &[(&str, &[Key], &[TextKey])] = &[
    ("panel-host", locks::KEYS, &[]),
    ("governance", governance::locks::KEYS, &[]),
    (
        "resources",
        resources::locks::KEYS,
        resources::locks::TEXT_KEYS,
    ),
    ("networking", &[], networking::locks::TEXT_KEYS),
    ("naming", naming::locks::KEYS, &[]),
    ("traffic", traffic::locks::KEYS, &[]),
    ("access", access::locks::KEYS, &[]),
    ("costs", costs::locks::KEYS, &[]),
    ("observability", observability::locks::KEYS, &[]),
    ("operations", operations::locks::KEYS, &[]),
];

fn crates() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
}

/// A number names one purpose, whichever form of key declares it: no two declarations
/// share one.
#[test]
fn every_lock_number_has_one_declaration() {
    let mut purposes: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (owner, keys, text_keys) in DECLARATIONS {
        let keys = keys.iter().map(|key| (key.number(), key.name()));
        let text_keys = text_keys.iter().map(|key| (key.number(), key.name()));
        for (number, name) in keys.chain(text_keys) {
            purposes
                .entry(number)
                .or_default()
                .push(format!("{owner}: {name}"));
        }
    }
    purposes.retain(|_, names| names.len() > 1);
    assert!(
        purposes.is_empty(),
        "lock numbers declared more than once: {purposes:#?}"
    );
}

/// Every crate with a `src/locks.rs` is listed above, and its lists name every key it
/// declares, so that the check of numbers sees all keys.
#[test]
fn every_declared_key_is_listed() {
    let mut declaring: Vec<String> = fs::read_dir(crates())
        .expect("crates directory")
        .map(|entry| entry.expect("crate directory").path())
        .filter(|path| path.join("src/locks.rs").is_file())
        .map(|path| {
            path.file_name()
                .expect("crate directory name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    declaring.sort();
    let mut listed: Vec<String> = DECLARATIONS
        .iter()
        .map(|(owner, ..)| owner.to_string())
        .collect();
    listed.sort();
    assert_eq!(listed, declaring, "crates that declare lock keys");
    for (owner, keys, text_keys) in DECLARATIONS {
        let source = fs::read_to_string(crates().join(owner).join("src/locks.rs"))
            .expect("key declarations");
        let declared = source
            .lines()
            .filter(|line| {
                line.starts_with("pub const ")
                    && (line.contains(": Key =") || line.contains(": TextKey ="))
            })
            .count();
        assert_eq!(
            declared,
            keys.len() + text_keys.len(),
            "{owner}/src/locks.rs lists every key it declares"
        );
    }
}

/// The database baseline and the scripts that adopt it take only declared keys.
#[test]
fn the_database_baseline_takes_only_declared_keys() {
    let root = crates().parent().expect("repository");
    let mut files = vec![root.join("crates/panel/migrations/0001_current.sql")];
    for entry in fs::read_dir(root.join("scripts")).expect("scripts directory") {
        let path = entry.expect("script").path();
        let name = path.file_name().expect("script name").to_string_lossy();
        if name.starts_with("baseline-adoption-") && name.ends_with(".sql") {
            files.push(path);
        }
    }
    let declared: Vec<Id> = DECLARATIONS
        .iter()
        .flat_map(|(_, keys, _)| keys.iter().map(|key| key.id()))
        .collect();
    let mut taken = 0;
    for file in &files {
        let sql = fs::read_to_string(file).expect("SQL file");
        let keys =
            locks::sql_keys(&sql).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for id in keys {
            assert!(
                declared.contains(&id),
                "{}: the advisory lock {id:?} is not declared",
                file.display()
            );
            taken += 1;
        }
    }
    assert!(taken > 0, "no advisory lock found in {files:?}");
}

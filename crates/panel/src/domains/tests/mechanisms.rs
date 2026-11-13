//! Small mechanisms with one implementation each: the audit trail is written by governance's
//! `control_center::audit`; passwords are hashed, random values drawn, short texts checked,
//! dates computed, cookies read and set, and the panel settings read and written by the
//! platform (`passwords`, `tokens`, `text`, `time`, `cookies`, `settings`). Each guard
//! names the one file allowed to hold the mechanism's tell-tale code; inline test modules
//! are left out, since fixtures prepare tables and requests directly.

use super::sources;

/// The source without its inline test module (`#[cfg(test)]` before `mod name {`), which
/// is the last item of a file. A test module in a file of its own (`mod tests;`) is skipped
/// by [`sources`] already.
fn production(source: &str) -> &str {
    for (at, attribute) in source.match_indices("#[cfg(test)]") {
        let declaration = source[at + attribute.len()..]
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("#["))
            .unwrap_or_default();
        let module = ["mod ", "pub mod ", "pub(crate) mod ", "pub(super) mod "]
            .iter()
            .any(|prefix| declaration.starts_with(prefix));
        if module && declaration.ends_with('{') {
            return &source[..at];
        }
    }
    source
}

/// Files, other than `owner`, whose production code contains any of `patterns`.
fn written_elsewhere(owner: &str, patterns: &[&str]) -> Vec<String> {
    sources()
        .into_iter()
        .filter(|(path, source)| {
            path != owner
                && patterns
                    .iter()
                    .any(|pattern| production(source).contains(pattern))
        })
        .map(|(path, _)| path)
        .collect()
}

#[test]
fn production_code_ends_at_the_inline_test_module() {
    let source =
        "#[cfg(test)]\nmod tests;\nfn kept() {}\n#[cfg(test)]\nmod tests {\n    fn left() {}\n}\n";
    assert_eq!(
        production(source),
        "#[cfg(test)]\nmod tests;\nfn kept() {}\n"
    );
    assert_eq!(production("fn only() {}\n"), "fn only() {}\n");
}

#[test]
fn the_audit_trail_is_written_by_governance_only() {
    let found = written_elsewhere(
        "governance/src/control_center/audit.rs",
        &["INSERT INTO management_audit", "UPDATE management_audit"],
    );
    assert!(
        found.is_empty(),
        "audit rows are written through apsp_governance::control_center::audit: {found:?}"
    );
}

#[test]
fn passwords_are_hashed_in_one_place() {
    let found = written_elsewhere("panel-host/src/passwords.rs", &["argon2::", "Argon2"]);
    assert!(
        found.is_empty(),
        "passwords are hashed and verified through apsp_panel_host::passwords: {found:?}"
    );
}

#[test]
fn random_values_are_drawn_in_one_place() {
    let found = written_elsewhere(
        "panel-host/src/tokens.rs",
        &["OsRng", "thread_rng", "rand::"],
    );
    assert!(
        found.is_empty(),
        "random tokens, codes, secrets and nonces come from apsp_panel_host::tokens: {found:?}"
    );
}

#[test]
fn control_characters_are_checked_in_one_place() {
    let found = written_elsewhere("panel-host/src/text.rs", &["is_control"]);
    assert!(
        found.is_empty(),
        "short texts are checked with apsp_panel_host::text: {found:?}"
    );
}

#[test]
fn calendar_arithmetic_is_done_in_one_place() {
    let found = written_elsewhere(
        "panel-host/src/time.rs",
        &["% 400", "719468", "719_468", "146097", "146_097"],
    );
    assert!(
        found.is_empty(),
        "dates and day numbers come from apsp_panel_host::time: {found:?}"
    );
}

#[test]
fn cookies_are_read_and_set_in_one_place() {
    let found = written_elsewhere(
        "panel-host/src/cookies.rs",
        &["header::COOKIE", "SET_COOKIE"],
    );
    assert!(
        found.is_empty(),
        "cookies are read and set through apsp_panel_host::cookies: {found:?}"
    );
}

#[test]
fn panel_settings_are_read_and_written_in_one_place() {
    let found = written_elsewhere(
        "panel-host/src/settings.rs",
        &[
            "FROM panel_settings",
            "UPDATE panel_settings",
            "INTO panel_settings",
        ],
    );
    assert!(
        found.is_empty(),
        "the panel settings are read and written through apsp_panel_host::settings: {found:?}"
    );
}

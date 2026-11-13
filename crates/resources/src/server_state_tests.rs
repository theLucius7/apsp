//! Guards for the server state questions resources answers once (ADR 0133): row locks on
//! `servers`, whether a server is being retired or in its own maintenance, and the keys of
//! its asset settings. They read the SQL in every panel crate, so a copy written anywhere
//! else fails `cargo test`; the exceptions name why they differ and may only shrink.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// Statements that lock server rows without the helpers in `servers.rs`.
const OTHER_ROW_LOCKS: [(&str, &str); 4] = [
    (
        "observability/src/telemetry/maintenance.rs",
        "claims a server whose history still has to be initialized with SKIP LOCKED: the \
         background pass takes whichever server is free and leaves a busy one to a later pass",
    ),
    (
        "traffic/src/publisher.rs",
        "claims a server whose publication is due with SKIP LOCKED, for the same reason",
    ),
    (
        "traffic/src/business.rs",
        "`mark_dirty` locks, in id order and in one statement, the servers it derives from the \
         ordered chains that share a touched server",
    ),
    (
        "traffic/src/nexus_nodes/mod.rs",
        "locks every live server for share while it checks that each legacy host tag maps to \
         exactly one server of the whole fleet",
    ),
];

/// The panel's Rust sources below `crates/`, as paths relative to it. Tests seed and inspect
/// every owner's tables directly, and the Agent side is another program with its own state.
fn panel_sources() -> Vec<(String, PathBuf)> {
    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("readable source directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().expect("file type").is_dir() {
                if !matches!(name.as_str(), "target" | "tests") {
                    walk(&path, found);
                }
            } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
                found.push(path);
            }
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory");
    let mut sources = Vec::new();
    for entry in fs::read_dir(crates).expect("readable crates directory") {
        let path = entry.expect("crate directory").path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_dir()
            && !matches!(name.as_str(), "agent" | "agent-core" | "tcp-probe")
            && !name.starts_with("adapter-")
        {
            walk(&path, &mut sources);
        }
    }
    let mut sources: Vec<_> = sources
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(crates)
                .expect("path under crates/")
                .to_string_lossy()
                .replace('\\', "/");
            (relative, path)
        })
        .collect();
    sources.sort();
    assert!(sources.len() > 100, "the workspace sources were not found");
    sources
}

/// The string literals of a Rust source (plain, byte and raw) with the line each starts on.
/// Comments, character literals and lifetimes are skipped; escapes stay as written, which is
/// enough to read SQL.
fn literals(source: &str) -> Vec<(usize, &str)> {
    let bytes = source.as_bytes();
    let ident =
        |at: usize| at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
    let (mut line, mut counted) = (1, 0);
    let mut line_at = |at: usize| {
        line += bytes[counted..at].iter().filter(|&&b| b == b'\n').count();
        counted = at;
        line
    };
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"//") {
            at += rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        } else if rest.starts_with(b"/*") {
            at += rest
                .windows(2)
                .position(|pair| pair == b"*/")
                .map_or(rest.len(), |end| end + 2);
        } else if !ident(at)
            && (rest.starts_with(b"r#") || rest.starts_with(b"r\"") || rest.starts_with(b"br"))
        {
            let prefix = if rest[0] == b'b' { 2 } else { 1 };
            let hashes = rest[prefix..].iter().take_while(|&&b| b == b'#').count();
            if rest.get(prefix + hashes) != Some(&b'"') {
                at += 1;
                continue;
            }
            let start = at + prefix + hashes + 1;
            let mut closing = vec![b'"'];
            closing.extend(std::iter::repeat_n(b'#', hashes));
            let end = bytes[start..]
                .windows(closing.len())
                .position(|window| window == closing.as_slice())
                .map_or(bytes.len(), |end| start + end);
            found.push((line_at(at), &source[start..end]));
            at = end + closing.len();
        } else if rest[0] == b'"' {
            let start = at + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = end.min(bytes.len());
            found.push((line_at(at), &source[start..end]));
            at = end + 1;
        } else if rest[0] == b'\'' {
            // A character literal, or the quote of a lifetime or label.
            at += if rest.get(1) == Some(&b'\\') {
                rest.iter()
                    .skip(3)
                    .position(|&b| b == b'\'')
                    .map_or(1, |end| end + 4)
            } else {
                let width = match rest.get(1) {
                    Some(&lead) if lead >= 0xf0 => 4,
                    Some(&lead) if lead >= 0xe0 => 3,
                    Some(&lead) if lead >= 0xc0 => 2,
                    _ => 1,
                };
                if rest.get(1 + width) == Some(&b'\'') {
                    width + 2
                } else {
                    1
                }
            };
        } else {
            at += 1;
        }
    }
    found
}

/// A literal as lowercase words and single punctuation marks; `s.id` stays one word.
fn words(sql: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in sql.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$') {
            word.push(c.to_ascii_lowercase());
        } else {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            if !c.is_whitespace() && c != '\\' {
                words.push(c.to_string());
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn sql(words: &[String]) -> bool {
    words.iter().any(|word| {
        matches!(
            word.as_str(),
            "select" | "insert" | "update" | "delete" | "with"
        )
    })
}

/// Whether a statement takes row locks on `servers`: it reads `servers` and has a locking
/// clause without `OF`, or one that names the table or its alias.
fn locks_servers(words: &[String]) -> bool {
    const AFTER_TABLE: [&str; 14] = [
        "where", "join", "left", "right", "inner", "full", "cross", "on", "order", "for", "group",
        "limit", "using", ",",
    ];
    let mut names = BTreeSet::new();
    for (at, pair) in words.windows(2).enumerate() {
        if matches!(pair[0].as_str(), "from" | "join") && pair[1] == "servers" {
            names.insert("servers".to_owned());
            let mut next = words.get(at + 2);
            if next.is_some_and(|word| word == "as") {
                next = words.get(at + 3);
            }
            if let Some(alias) = next.filter(|word| {
                !AFTER_TABLE.contains(&word.as_str())
                    && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            }) {
                names.insert(alias.clone());
            }
        }
    }
    if names.is_empty() {
        return false;
    }
    let mut at = 0;
    while at + 1 < words.len() {
        let strength = match (words[at].as_str(), words[at + 1].as_str()) {
            ("for", "update" | "share") => 2,
            ("for", "no" | "key")
                if words
                    .get(at + 2)
                    .is_some_and(|word| word == "key" || word == "share") =>
            {
                if words[at + 1] == "no" { 4 } else { 3 }
            }
            _ => {
                at += 1;
                continue;
            }
        };
        let mut next = at + strength;
        if words.get(next).is_none_or(|word| word != "of") {
            return true;
        }
        next += 1;
        while let Some(table) = words.get(next) {
            if names.contains(table) {
                return true;
            }
            if words.get(next + 1).is_some_and(|word| word == ",") {
                next += 2;
            } else {
                break;
            }
        }
        at = next;
    }
    false
}

/// An SQL literal of the panel sources.
struct Statement {
    file: String,
    line: usize,
    words: Vec<String>,
    text: String,
}

/// The SQL literals of the panel sources, read once for all guards.
fn statements() -> &'static [Statement] {
    static STATEMENTS: OnceLock<Vec<Statement>> = OnceLock::new();
    STATEMENTS.get_or_init(|| {
        let mut statements = Vec::new();
        for (file, path) in panel_sources() {
            let source = fs::read_to_string(&path).expect("readable source");
            for (line, literal) in literals(&source) {
                let words = words(literal);
                if sql(&words) {
                    statements.push(Statement {
                        file: file.clone(),
                        line,
                        words,
                        text: literal.split_whitespace().collect::<Vec<_>>().join(" "),
                    });
                }
            }
        }
        statements
    })
}

/// The SQL literals that match, outside the files allowed to write them, as the file and
/// `path:line: text`.
fn misplaced(allowed: &[&str], matches: impl Fn(&[String]) -> bool) -> Vec<(&'static str, String)> {
    statements()
        .iter()
        .filter(|statement| {
            !allowed.contains(&statement.file.as_str()) && matches(&statement.words)
        })
        .map(|statement| {
            (
                statement.file.as_str(),
                format!("{}:{}: {}", statement.file, statement.line, statement.text),
            )
        })
        .collect()
}

/// A server row lock taken anywhere but `servers.rs` would be a second lock with its own
/// filter and strength; per-server work serializes on `servers::lock`, `lock_row` and
/// `lock_all`.
#[test]
fn server_row_locks_are_taken_through_resources() {
    let found = misplaced(&["resources/src/servers.rs"], locks_servers);
    let listed: BTreeSet<_> = OTHER_ROW_LOCKS.iter().map(|(file, _)| *file).collect();
    let used: BTreeSet<_> = found
        .iter()
        .map(|(file, _)| *file)
        .filter(|file| listed.contains(file))
        .collect();
    let unexpected: Vec<_> = found
        .iter()
        .filter(|(file, _)| !listed.contains(file))
        .map(|(_, line)| line.as_str())
        .collect();
    assert!(
        unexpected.is_empty(),
        "lock server rows with apsp_resources::servers::lock, lock_row or lock_all:\n{}",
        unexpected.join("\n")
    );
    assert_eq!(
        used, listed,
        "remove exceptions that no longer lock server rows"
    );
}

/// Retirement and the maintenance lifecycle are read through `retirement::retiring_sql` and
/// `server_lifecycle::{blocks_work_sql, maintenance_sql}`, never written out again.
#[test]
fn server_lifecycle_is_read_through_resources() {
    let found = misplaced(
        &[
            "resources/src/retirement.rs",
            "resources/src/server_lifecycle.rs",
        ],
        |words| {
            words.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "server_retirements" | "maintenance_from" | "maintenance_until" | "draining"
                )
            })
        },
    );
    let lines: Vec<_> = found.iter().map(|(_, line)| line.as_str()).collect();
    assert!(
        lines.is_empty(),
        "ask apsp_resources::retirement::retiring_sql or server_lifecycle::blocks_work_sql / \
         maintenance_sql:\n{}",
        lines.join("\n")
    );
}

/// Asset settings keys are read through `server_assets::sql`, which applies the typed
/// defaults; a key read by hand would choose its own.
#[test]
fn asset_settings_keys_are_read_through_resources() {
    let found = misplaced(&["resources/src/server_assets.rs"], |words| {
        words.windows(2).any(|pair| {
            pair[0].ends_with("asset_settings") && matches!(pair[1].as_str(), "-" | "#")
        })
    });
    let lines: Vec<_> = found.iter().map(|(_, line)| line.as_str()).collect();
    assert!(
        lines.is_empty(),
        "read asset settings with apsp_resources::server_assets::sql:\n{}",
        lines.join("\n")
    );
}

#[test]
fn the_guards_read_sql_as_written() {
    let source = "let a = \"SELECT id FROM servers s WHERE s.id=$1 FOR UPDATE OF s\"; // \"x\"\n\
                  let b = r#\"SELECT x FROM nodes n JOIN servers s ON s.id=n.server_id FOR UPDATE OF n\"#;\n\
                  let c = '\"'; let d: &'static str = \"SELECT id FROM servers FOR SHARE SKIP LOCKED\";";
    let found = literals(source);
    assert_eq!(found.len(), 3);
    assert_eq!(found[1].0, 2);
    assert!(locks_servers(&words(found[0].1)));
    assert!(!locks_servers(&words(found[1].1)));
    assert!(locks_servers(&words(found[2].1)));
    assert!(locks_servers(&words(
        "SELECT 1 FROM servers WHERE id=$1 FOR NO KEY UPDATE"
    )));
    assert!(!locks_servers(&words(
        "SELECT 1 FROM server_alert_events e JOIN servers s ON s.id=e.server_id FOR UPDATE OF e"
    )));
    assert!(sql(&words("UPDATE x SET a=1")));
    assert!(!sql(&words("lifecycle")));
}

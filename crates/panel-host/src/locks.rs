//! PostgreSQL advisory locks, held until the transaction that takes them ends.
//!
//! Every advisory lock the panel takes goes through a [`Key`] or a [`TextKey`]. The owner of a
//! lock declares its key once, in the `locks` module of its crate (`src/locks.rs`), with the
//! number the lock has always used and what it is for, and lists it in that module's `KEYS`
//! (text keys in `TEXT_KEYS`). A number stays as it is once used: running transactions and the
//! database baseline's SQL functions take the same locks. The application checks all
//! declarations together: each number has one declaration, and the database baseline and the
//! scripts that adopt it take only declared keys. A test here keeps advisory lock SQL in this
//! module and key declarations in the `locks` modules.

use sqlx::PgConnection;

/// Initializing or checking the database baseline ([`crate::schema`]). The scripts that move a
/// database to a new baseline (`scripts/baseline-adoption-*.sql`) take it too.
pub const BASELINE: Key = Key::new(73401220, "database baseline");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[BASELINE];

/// What PostgreSQL locks. The two forms never take each other's locks, whatever the numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Id {
    /// One 64-bit key: `pg_advisory_xact_lock(bigint)`.
    Single(i64),
    /// A class and an object in it: `pg_advisory_xact_lock(integer, integer)`.
    Pair(i32, i32),
}

/// An advisory lock: what PostgreSQL locks and what the lock is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    id: Id,
    name: &'static str,
}

impl Key {
    /// A 64-bit key.
    pub const fn new(key: i64, name: &'static str) -> Self {
        Self {
            id: Id::Single(key),
            name,
        }
    }

    /// A pair: a class and an object in it.
    pub const fn pair(class: i32, object: i32, name: &'static str) -> Self {
        Self {
            id: Id::Pair(class, object),
            name,
        }
    }

    /// The key `by` places further, for locks taken per object: the 64-bit key plus `by`, or
    /// the same class with the object plus `by`. Both stop at the end of their range, where
    /// the remaining objects share one lock. Counting up from a 64-bit key can reach the keys
    /// above it, while a class keeps its objects to itself, so new locks per object belong in
    /// a class of their own.
    pub fn offset(self, by: i64) -> Self {
        let id = match self.id {
            Id::Single(key) => Id::Single(key.saturating_add(by)),
            Id::Pair(class, object) => {
                let object = i64::from(object).saturating_add(by);
                Id::Pair(class, object.clamp(i32::MIN.into(), i32::MAX.into()) as i32)
            }
        };
        Self { id, ..self }
    }

    /// What PostgreSQL locks.
    pub const fn id(self) -> Id {
        self.id
    }

    /// The number that names the key among all declarations: the 64-bit key or the class.
    pub fn number(self) -> i64 {
        match self.id {
            Id::Single(key) => key,
            Id::Pair(class, _) => class.into(),
        }
    }

    /// What the lock is for, for diagnostics.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Waits until the transaction on `connection` holds the lock, which lasts until the
    /// transaction ends.
    pub async fn lock(self, connection: &mut PgConnection) -> sqlx::Result<()> {
        match self.id {
            Id::Single(key) => {
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(key)
                    .execute(connection)
                    .await?
            }
            Id::Pair(class, object) => {
                sqlx::query("SELECT pg_advisory_xact_lock($1,$2)")
                    .bind(class)
                    .bind(object)
                    .execute(connection)
                    .await?
            }
        };
        Ok(())
    }

    /// Takes the lock for the transaction on `connection` unless another transaction holds
    /// it, which `false` reports.
    pub async fn try_lock(self, connection: &mut PgConnection) -> sqlx::Result<bool> {
        match self.id {
            Id::Single(key) => {
                sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
                    .bind(key)
                    .fetch_one(connection)
                    .await
            }
            Id::Pair(class, object) => {
                sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1,$2)")
                    .bind(class)
                    .bind(object)
                    .fetch_one(connection)
                    .await
            }
        }
    }
}

/// Advisory locks named by text, one per text: PostgreSQL hashes the text with the seed into
/// the 64-bit space (`hashtextextended`), and different seeds keep families of texts apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextKey {
    seed: i64,
    name: &'static str,
}

impl TextKey {
    /// Text keys hashed with `seed`.
    pub const fn new(seed: i64, name: &'static str) -> Self {
        Self { seed, name }
    }

    /// The number that names the key among all declarations: its seed.
    pub const fn number(self) -> i64 {
        self.seed
    }

    /// What the locks are for, for diagnostics.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Waits until the transaction on `connection` holds the lock of `text`, which lasts until
    /// the transaction ends.
    pub async fn lock(self, connection: &mut PgConnection, text: &str) -> sqlx::Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,$2))")
            .bind(text)
            .bind(self.seed)
            .execute(connection)
            .await?;
        Ok(())
    }

    /// Takes the lock of `text` for the transaction on `connection` unless another transaction
    /// holds it, which `false` reports.
    pub async fn try_lock(self, connection: &mut PgConnection, text: &str) -> sqlx::Result<bool> {
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,$2))")
            .bind(text)
            .bind(self.seed)
            .fetch_one(connection)
            .await
    }
}

/// The keys that SQL kept outside Rust takes, such as the database baseline's functions and
/// the scripts that adopt a baseline. There every advisory lock call with arguments names its
/// key with literal numbers, one for a 64-bit key and two for a pair, so that the declarations
/// can be checked against it; any other argument is an error.
pub fn sql_keys(sql: &str) -> Result<Vec<Id>, String> {
    let mut keys = Vec::new();
    for (start, _) in sql.match_indices("pg_") {
        let call = &sql[start..];
        let (function, rest) = call.split_at(
            call.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(call.len()),
        );
        let Some(arguments) = rest.trim_start().strip_prefix('(') else {
            continue;
        };
        if !function.contains("advisory") {
            continue;
        }
        let end = arguments
            .find(')')
            .ok_or_else(|| format!("{function}: the call is not closed"))?;
        let arguments = &arguments[..end];
        if arguments.trim().is_empty() {
            continue;
        }
        let numbers: Vec<Option<i64>> = arguments
            .split(',')
            .map(|number| number.trim().parse().ok())
            .collect();
        let key = match numbers.as_slice() {
            [Some(key)] => Some(Id::Single(*key)),
            [Some(class), Some(object)] => i32::try_from(*class)
                .ok()
                .zip(i32::try_from(*object).ok())
                .map(|(class, object)| Id::Pair(class, object)),
            _ => None,
        };
        keys.push(key.ok_or_else(|| {
            format!("{function}({arguments}) does not name its key with literal numbers")
        })?);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::{Id, Key, TextKey, sql_keys};
    use sqlx::PgPool;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[test]
    fn offsets_count_objects_from_the_declared_key() {
        let key = Key::new(73002000, "fixture");
        assert_eq!(key.offset(1).id(), Id::Single(73002001));
        assert_eq!(key.offset(1).name(), "fixture");
        assert_eq!(
            Key::new(i64::MAX, "fixture").offset(1).id(),
            Id::Single(i64::MAX)
        );
        let class = Key::pair(7, 0, "fixture");
        assert_eq!(class.offset(-5).id(), Id::Pair(7, -5));
        assert_eq!(class.offset(i64::MIN).id(), Id::Pair(7, i32::MIN));
        assert_eq!(class.offset(i64::MAX).id(), Id::Pair(7, i32::MAX));
        assert_eq!(class.offset(3).number(), 7);
        assert_eq!(TextKey::new(9, "fixture").number(), 9);
    }

    #[test]
    fn sql_names_its_keys_with_literal_numbers() {
        let sql = "SELECT pg_advisory_xact_lock(73401220);\n\
            PERFORM pg_try_advisory_xact_lock ( 7, -1 );\n\
            SELECT pg_advisory_unlock_all(); SELECT count(*) FROM pg_class;";
        assert_eq!(
            sql_keys(sql),
            Ok(vec![Id::Single(73401220), Id::Pair(7, -1)])
        );
        for sql in [
            "PERFORM pg_advisory_xact_lock(hashtextextended(name, 0));",
            "PERFORM pg_advisory_xact_lock(key);",
            "PERFORM pg_advisory_xact_lock(4294967296, 1);",
            "PERFORM pg_advisory_xact_lock(1",
        ] {
            assert!(sql_keys(sql).is_err(), "{sql}");
        }
    }

    /// A key takes the lock its literal SQL took: a bound number is the literal number, a pair
    /// never meets a 64-bit key, and a text key is `hashtextextended` of the text and the seed.
    #[sqlx::test(migrations = false)]
    async fn keys_take_the_locks_of_their_literal_numbers(pool: PgPool) -> sqlx::Result<()> {
        let mut holder = pool.begin().await?;
        Key::new(42, "fixture").lock(&mut holder).await?;
        Key::pair(7, 1, "fixture").lock(&mut holder).await?;
        TextKey::new(9, "fixture")
            .lock(&mut holder, "fixture text")
            .await?;
        let mut other = pool.begin().await?;
        for (probe, free) in [
            ("SELECT pg_try_advisory_xact_lock(42)", false),
            ("SELECT pg_try_advisory_xact_lock(7,1)", false),
            (
                "SELECT pg_try_advisory_xact_lock(hashtextextended('fixture text',9))",
                false,
            ),
            ("SELECT pg_try_advisory_xact_lock(0,42)", true),
            ("SELECT pg_try_advisory_xact_lock(7,2)", true),
            (
                "SELECT pg_try_advisory_xact_lock(hashtextextended('fixture text',0))",
                true,
            ),
        ] {
            let taken: bool = sqlx::query_scalar(probe).fetch_one(&mut *other).await?;
            assert_eq!(taken, free, "{probe}");
        }
        assert!(!Key::new(42, "fixture").try_lock(&mut other).await?);
        assert!(
            !Key::pair(7, 0, "fixture")
                .offset(1)
                .try_lock(&mut other)
                .await?
        );
        assert!(
            !TextKey::new(9, "fixture")
                .try_lock(&mut other, "fixture text")
                .await?
        );
        holder.commit().await?;
        assert!(Key::new(42, "fixture").try_lock(&mut other).await?);
        Ok(())
    }

    /// The Rust sources below `crates/`, without build output.
    fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("readable source directory") {
            let entry = entry.expect("directory entry");
            let kind = entry.file_type().expect("file type");
            let path = entry.path();
            if kind.is_dir() && entry.file_name() != "target" {
                sources(&path, found);
            } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs")
            {
                found.push(path);
            }
        }
    }

    /// Whether a line declares a key (`Key::new(`, `Key::pair(`, `TextKey::new(`), as opposed
    /// to calling a constructor of another type whose name ends in `Key`.
    fn declares_key(line: &str) -> bool {
        line.match_indices("Key::").any(|(at, _)| {
            let before = &line[..at];
            let name = &before[before
                .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
                .len()..];
            let call = &line[at + "Key::".len()..];
            matches!(name, "" | "Text") && (call.starts_with("new(") || call.starts_with("pair("))
        })
    }

    /// Advisory lock SQL is written only in this module and keys are declared only in a
    /// crate's `src/locks.rs`; anywhere else a number would bypass the declarations.
    #[test]
    fn advisory_locks_go_through_declared_keys() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates directory");
        let mut files = Vec::new();
        sources(crates, &mut files);
        assert!(files.len() > 100, "the workspace sources were not found");
        let mut misplaced = Vec::new();
        for path in &files {
            let mechanism = path.ends_with("panel-host/src/locks.rs");
            let declarations = path.ends_with("src/locks.rs");
            let source = fs::read_to_string(path).expect("readable source");
            for (index, line) in source.lines().enumerate() {
                let sql = line.contains("pg_advisory") || line.contains("pg_try_advisory");
                if (sql && !mechanism) || (declares_key(line) && !declarations) {
                    misplaced.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
                }
            }
        }
        assert!(
            misplaced.is_empty(),
            "take advisory locks through a key declared in the owner's src/locks.rs \
             (apsp_panel_host::locks):\n{}",
            misplaced.join("\n")
        );
        assert!(declares_key(
            "pub const A: TextKey = TextKey::new(1, \"a\");"
        ));
        assert!(!declares_key("UnboundKey::new(&AES_256_GCM, bytes)"));
    }
}

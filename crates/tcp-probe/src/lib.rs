#![forbid(unsafe_code)]

mod cli;
mod engine;
mod journal;
mod model;

pub use cli::{Command, Options, parse};
pub use engine::{run, run_until};
pub use journal::Journal;
pub use model::{IpVersion, Report, Snapshot, Target};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SOURCE_COMMIT: Option<&str> = option_env!("APSP_NATIVE_TCP_SOURCE_COMMIT");
const _: () = {
    if let Some(commit) = SOURCE_COMMIT {
        let bytes = commit.as_bytes();
        assert!(
            bytes.len() == 40,
            "source commit must have 40 hexadecimal digits"
        );
        let mut index = 0;
        while index < bytes.len() {
            assert!(
                matches!(bytes[index], b'0'..=b'9' | b'a'..=b'f'),
                "source commit must use lowercase hexadecimal digits"
            );
            index += 1;
        }
    }
};
pub const INPUT_LIMIT: usize = 16 * 1024;
pub const OUTPUT_LIMIT: usize = 64 * 1024;

#[cfg(test)]
mod tests;

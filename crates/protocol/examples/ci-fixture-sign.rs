#![forbid(unsafe_code)]

// Reuse the existing public TEST_ONLY key and signing implementation.
#[path = "../tests/support/release.rs"]
mod test_release;

use apsp_protocol::release::MAX_CHECKSUMS_BYTES;
use std::io::{Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args_os().count() != 1 {
        return Err("TEST_ONLY signer accepts checksums on stdin and no arguments".into());
    }
    let mut checksums = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_CHECKSUMS_BYTES + 1) as u64)
        .read_to_end(&mut checksums)?;
    if checksums.is_empty() || checksums.len() > MAX_CHECKSUMS_BYTES {
        return Err("TEST_ONLY checksums are empty or exceed the release limit".into());
    }
    std::io::stdout()
        .lock()
        .write_all(test_release::sign(&checksums).as_bytes())?;
    Ok(())
}

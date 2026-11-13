#![forbid(unsafe_code)]

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}

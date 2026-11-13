#![forbid(unsafe_code)]

fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");
    assert!(
        std::path::Path::new("../../web/dist/index.html").is_file(),
        "Frontend assets are missing. Run `cd web && bun install --frozen-lockfile && bun run build` before building the panel."
    );
    println!("cargo:rerun-if-changed=../panel/migrations");
}

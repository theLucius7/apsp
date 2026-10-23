#!/usr/bin/env bash
# Build a native, statically linked Linux Agent artifact.
set -euo pipefail
umask 022

usage() {
  cat <<'USAGE'
Usage: tools/build-agent.sh <amd64|arm64> <ARTIFACT_ROOT>

Run on a Linux machine of the requested architecture with Rust stable, rustup,
python3, musl-tools, build-essential, binutils, and coreutils. Add the matching
Rust musl target first:
  rustup target add x86_64-unknown-linux-musl   # amd64
  rustup target add aarch64-unknown-linux-musl # arm64

Output: ARTIFACT_ROOT/agent/<agent-version>/<arch> (a raw static ELF)
        ARTIFACT_ROOT/agent/<agent-version>/SHA256SUMS

The script verifies ELF architecture, absence of dynamic dependencies, and
native --version/--help execution. Existing architecture files are immutable;
the other architecture's verified checksum is retained when adding a build.
USAGE
}
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; exit 0; fi
[[ $# == 2 ]] || { usage >&2; exit 2; }
arch=$1
case "$arch" in
  amd64) target=x86_64-unknown-linux-musl; machine=x86_64 ;;
  arm64) target=aarch64-unknown-linux-musl; machine=aarch64 ;;
  *) die 'architecture must be amd64 or arm64' ;;
esac
[[ -n $2 ]] || die 'ARTIFACT_ROOT must not be empty'
for tool in cargo rustc rustup python3 musl-gcc readelf sha256sum timeout; do
  command -v "$tool" >/dev/null || die "missing build tool: $tool"
done
[[ $(uname -s) == Linux && $(uname -m) == "$machine" ]] || die "use a native Linux/$arch build host"
rustup target list --installed | grep -Fxq "$target" || die "run: rustup target add $target"
[[ -z ${CARGO_ENCODED_RUSTFLAGS:-} ]] || die 'unset CARGO_ENCODED_RUSTFLAGS so the static-link flag can be applied'
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
[[ -f $repo/Cargo.lock && -f $repo/crates/agent/Cargo.toml ]] || die 'run this script from its checked-out tools directory'
metadata=$(cargo metadata --locked --no-deps --format-version 1 --manifest-path "$repo/Cargo.toml")
version=$(printf '%s' "$metadata" | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "apsp-agent"))')
target_dir=$(printf '%s' "$metadata" | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || die 'invalid Agent package version'
output=$2/agent/$version
[[ ! -L $output ]] || die 'output version directory must not be a symlink'
mkdir -p "$output"
output=$(cd "$output" && pwd -P)
lock=$output/.build.lock
mkdir "$lock" 2>/dev/null || die "another build owns $lock; remove only after confirming that build has stopped"
stage_file=
sums_file=
output_created=0
committed=0
cleanup() {
  if [[ $output_created == 1 && $committed == 0 ]]; then rm -f -- "$output/$arch"; fi
  [[ -z $stage_file ]] || rm -f -- "$stage_file"
  [[ -z $sums_file ]] || rm -f -- "$sums_file"
  rmdir -- "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
[[ ! -e $output/$arch && ! -L $output/$arch ]] || die "immutable artifact already exists: $output/$arch"
python3 - "$output" <<'PY'
import hashlib
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
manifest = root / "SHA256SUMS"
expected = {}
if manifest.is_symlink() or (manifest.exists() and not manifest.is_file()):
    raise SystemExit("SHA256SUMS must be an ordinary file")
if manifest.exists():
    for line in manifest.read_text().splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](amd64|arm64|linux-(?:gnu|musl)-(?:amd64|arm64)|macos-arm64|(?:windows|freebsd)-(?:amd64|arm64))", line)
        if not match or match[2] in expected:
            raise SystemExit("invalid or duplicate SHA256SUMS entry")
        expected[match[2]] = match[1].lower()
for arch in ("amd64", "arm64", "linux-musl-amd64", "linux-musl-arm64", "linux-gnu-amd64", "linux-gnu-arm64", "macos-arm64", "windows-amd64", "windows-arm64", "freebsd-amd64", "freebsd-arm64"):
    artifact = root / arch
    if artifact.is_symlink() or (artifact.exists() and not artifact.is_file()):
        raise SystemExit(f"{arch} must be an ordinary file")
    if artifact.exists():
        digest = hashlib.sha256()
        with artifact.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        actual = digest.hexdigest()
        if expected.get(arch) != actual:
            raise SystemExit(f"existing {arch} lacks a matching checksum; refusing to replace the manifest")
    elif arch in expected:
        raise SystemExit(f"SHA256SUMS refers to missing {arch}")
PY

target_key=${target//-/_}
linker_key=$(printf '%s' "$target_key" | tr '[:lower:]' '[:upper:]')
export "CARGO_TARGET_${linker_key}_LINKER=musl-gcc"
export "CC_${target_key}=musl-gcc"
export "RUSTFLAGS=${RUSTFLAGS:+$RUSTFLAGS }-Ctarget-feature=+crt-static"
cargo build --locked --release --package apsp-agent --target "$target" --manifest-path "$repo/Cargo.toml"
binary=$target_dir/$target/release/apsp-agent
header=$(readelf -h "$binary")
if [[ $arch == amd64 ]]; then
  [[ $header == *'Advanced Micro Devices X86-64'* ]] || die 'built binary has the wrong architecture'
else
  [[ $header == *AArch64* ]] || die 'built binary has the wrong architecture'
fi
program_headers=$(readelf -lW "$binary")
dynamic_entries=$(readelf -dW "$binary")
[[ $program_headers != *INTERP* && $dynamic_entries != *NEEDED* ]] || die 'Agent is not statically linked'
[[ $(timeout 30 "$binary" --version) == "apsp-agent $version" ]] || die 'Agent version verification failed'
timeout 30 "$binary" --help
python3 - "$repo/tools" "$output" "$arch" "$binary" <<'PY_PUBLISH'
import pathlib
import sys
sys.path.insert(0, sys.argv[1])
from artifact_manifest import publish
root, arch, binary = pathlib.Path(sys.argv[2]), sys.argv[3], pathlib.Path(sys.argv[4])
publish(root, arch, binary.read_bytes())
publish(root, f"linux-musl-{arch}", binary.read_bytes())
(root / arch).chmod(0o755)
(root / f"linux-musl-{arch}").chmod(0o755)
PY_PUBLISH
committed=1
printf 'Artifact: %s/%s\n' "$output" "$arch"
cat "$output/SHA256SUMS"

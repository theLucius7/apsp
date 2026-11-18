#!/usr/bin/env bash
# Build a native, statically linked self-contained Linux Agent artifact.
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

Inputs: ARTIFACT_ROOT/sing-box/<singbox-version>/linux-musl-<arch> (or APSP_EMBEDDED_SINGBOX_ARCHIVE)
        When APSP_RELEASE_EMBED_REQUIRED=1, also requires:
          ARTIFACT_ROOT/nodequality/<version>/<arch>
          ARTIFACT_ROOT/ipquality/<version>/<arch>
          ARTIFACT_ROOT/network-workbench/<version>/<arch>

Output: ARTIFACT_ROOT/agent/<agent-version>/<arch> (a raw static ELF <= 128 MiB)
        ARTIFACT_ROOT/agent/<agent-version>/linux-musl-<arch>
        ARTIFACT_ROOT/agent/<agent-version>/SHA256SUMS

The script verifies ELF architecture, absence of dynamic dependencies, raw
128 MiB updater size ceiling, and native --version/--help/tcp-probe execution.
Existing architecture files are immutable; the other architecture's verified
checksum is retained when adding a build.
USAGE
}
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
resolve_ordinary_archive() {
  local candidate=$1 label=$2
  [[ -f $candidate && ! -L $candidate && -s $candidate ]] || die "missing or invalid $label archive: $candidate"
  printf '%s/%s\n' "$(cd "$(dirname "$candidate")" && pwd -P)" "$(basename "$candidate")"
}
if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; exit 0; fi
[[ $# == 2 ]] || { usage >&2; exit 2; }
arch=$1
case "$arch" in
  amd64) target=x86_64-unknown-linux-musl; machine=x86_64 ;;
  arm64) target=aarch64-unknown-linux-musl; machine=aarch64 ;;
  *) die 'architecture must be amd64 or arm64' ;;
esac
[[ -n $2 ]] || die 'ARTIFACT_ROOT must not be empty'
for tool in cargo rustc rustup python3 musl-gcc readelf sha256sum stat timeout; do
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

release_embed_required=0
case "${APSP_RELEASE_EMBED_REQUIRED:-}" in
  1|[Tt][Rr][Uu][Ee]|[Yy][Ee][Ss]|[Oo][Nn]) release_embed_required=1 ;;
esac

singbox_version=${APSP_EMBEDDED_SINGBOX_VERSION:-1.14.4-apsp.2}
singbox_candidate=${APSP_EMBEDDED_SINGBOX_ARCHIVE:-$2/sing-box/$singbox_version/linux-musl-$arch}
export APSP_EMBEDDED_SINGBOX_VERSION=$singbox_version
export APSP_EMBEDDED_SINGBOX_ARCHIVE=$(resolve_ordinary_archive "$singbox_candidate" "sing-box")

nodequality_version=${APSP_EMBEDDED_NODEQUALITY_VERSION:-a92fca6c0067df29ddd03fdc2fee6f3000f64545-r22}
nodequality_candidate=${APSP_EMBEDDED_NODEQUALITY_ARCHIVE:-$2/nodequality/$nodequality_version/$arch}
if [[ -n ${APSP_EMBEDDED_NODEQUALITY_ARCHIVE:-} || $release_embed_required == 1 || -e $nodequality_candidate || -L $nodequality_candidate ]]; then
  export APSP_EMBEDDED_NODEQUALITY_VERSION=$nodequality_version
  export APSP_EMBEDDED_NODEQUALITY_ARCHIVE=$(resolve_ordinary_archive "$nodequality_candidate" "nodequality")
fi

ipquality_version=${APSP_EMBEDDED_IPQUALITY_VERSION:-87397e2c3196ec796f5477c83343c2354df601ea-node-r1}
ipquality_candidate=${APSP_EMBEDDED_IPQUALITY_ARCHIVE:-$2/ipquality/$ipquality_version/$arch}
if [[ -n ${APSP_EMBEDDED_IPQUALITY_ARCHIVE:-} || $release_embed_required == 1 || -e $ipquality_candidate || -L $ipquality_candidate ]]; then
  export APSP_EMBEDDED_IPQUALITY_VERSION=$ipquality_version
  export APSP_EMBEDDED_IPQUALITY_ARCHIVE=$(resolve_ordinary_archive "$ipquality_candidate" "ipquality")
fi

workbench_version=${APSP_EMBEDDED_NETWORK_WORKBENCH_VERSION:-1.0.0}
workbench_candidate=${APSP_EMBEDDED_NETWORK_WORKBENCH_ARCHIVE:-$2/network-workbench/$workbench_version/$arch}
if [[ -n ${APSP_EMBEDDED_NETWORK_WORKBENCH_ARCHIVE:-} || $release_embed_required == 1 || -e $workbench_candidate || -L $workbench_candidate ]]; then
  export APSP_EMBEDDED_NETWORK_WORKBENCH_VERSION=$workbench_version
  export APSP_EMBEDDED_NETWORK_WORKBENCH_ARCHIVE=$(resolve_ordinary_archive "$workbench_candidate" "network-workbench")
fi

if [[ -z ${APSP_NATIVE_TCP_SOURCE_COMMIT:-} ]] && command -v git >/dev/null; then
  if git_head=$(git -C "$repo" rev-parse --verify HEAD 2>/dev/null) && [[ $git_head =~ ^[0-9a-f]{40}$ ]]; then
    export APSP_NATIVE_TCP_SOURCE_COMMIT=$git_head
  fi
fi

target_key=${target//-/_}
linker_key=$(printf '%s' "$target_key" | tr '[:lower:]' '[:upper:]')
export "CARGO_TARGET_${linker_key}_LINKER=musl-gcc"
export "CC_${target_key}=musl-gcc"
export "RUSTFLAGS=${RUSTFLAGS:+$RUSTFLAGS }-Ctarget-feature=+crt-static"
cargo build --locked --release --package apsp-agent --target "$target" --manifest-path "$repo/Cargo.toml"
binary=$target_dir/$target/release/apsp-agent
binary_size=$(stat -c '%s' "$binary")
(( binary_size > 0 && binary_size <= 134217728 )) || die "Agent binary size ($binary_size bytes) exceeds 128 MiB raw update limit"
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
[[ $(timeout 30 "$binary" tcp-probe --version) == apsp-tcp-probe\ * ]] || die 'built-in tcp-probe verification failed'
timeout 30 "$binary" --help
python3 - "$repo/tools" "$output" "$arch" "$binary" "$version" "$release_embed_required" "$singbox_version" "$nodequality_version" "$ipquality_version" "$workbench_version" <<'PY_PUBLISH'
import pathlib
import sys

sys.path.insert(0, sys.argv[1])
from artifact_manifest import publish
from release import verify_embedded_agent_manifest

root, arch, binary = pathlib.Path(sys.argv[2]), sys.argv[3], pathlib.Path(sys.argv[4])
expected_version, release_required = sys.argv[5], sys.argv[6] == "1"
expected_versions = {
    "sing-box": sys.argv[7],
    "nodequality": sys.argv[8],
    "ipquality": sys.argv[9],
    "network-workbench": sys.argv[10],
}
payload = binary.read_bytes()
verify_embedded_agent_manifest(
    payload,
    expected_version,
    arch,
    require_release_closure=release_required,
    expected_versions=expected_versions,
)
publish(root, arch, payload)
publish(root, f"linux-musl-{arch}", payload)
(root / arch).chmod(0o755)
(root / f"linux-musl-{arch}").chmod(0o755)
PY_PUBLISH
committed=1
printf 'Artifact: %s/%s\n' "$output" "$arch"
cat "$output/SHA256SUMS"

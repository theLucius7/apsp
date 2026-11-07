#!/usr/bin/env bash
# Build the stable upstream kernel with APSP limits from the pinned upstream tag plus the Apsp
# patch queue in tools/singbox-runtime (see its README).
set -euo pipefail
umask 022

usage() {
  cat <<'USAGE'
Usage: tools/build-singbox.sh <amd64|arm64> <ARTIFACT_ROOT> [--libc=gnu|--libc=musl]

Run on Linux/amd64 with Go 1.26.8. The runtime is pure Go (CGO disabled);
one static binary serves glibc and musl hosts;
--libc only selects the artifact name.

Output: ARTIFACT_ROOT/sing-box/1.14.4-apsp.1/<arch> (a gzip archive with one binary)
        ARTIFACT_ROOT/sing-box/1.14.4-apsp.1/SHA256SUMS
With --libc, the artifact filename is linux-<libc>-<arch>. The legacy default
remains GNU under <arch>.

Debian build prerequisites:
  ca-certificates git python3 binutils coreutils

Existing architecture files are immutable. A second invocation for the other
architecture verifies and retains the existing checksum entry. Do not run both
architectures concurrently against the same output directory.
USAGE
}
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; exit 0; fi
[[ $# == 2 || $# == 3 ]] || { usage >&2; exit 2; }
arch=$1
case "$arch" in amd64|arm64) ;; *) die 'architecture must be amd64 or arm64' ;; esac
libc=gnu
artifact=$arch
if [[ $# == 3 ]]; then
  case "$3" in --libc=gnu) libc=gnu ;; --libc=musl) libc=musl ;; *) die 'expected --libc=gnu or --libc=musl' ;; esac
  artifact=linux-$libc-$arch
fi
[[ -n $2 ]] || die 'ARTIFACT_ROOT must not be empty'
for tool in go git python3 tar gzip sha256sum readelf timeout; do
  command -v "$tool" >/dev/null || die "missing build tool: $tool"
done
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || die 'use a Linux/amd64 build host (both targets are supported there)'
export GOTOOLCHAIN=local
[[ $(go env GOVERSION) == go1.26.8 ]] || die 'Go 1.26.8 is required'
[[ $(go env GOHOSTOS)/$(go env GOHOSTARCH) == linux/amd64 ]] || die 'Go must run natively on Linux/amd64'
tar_version=$(tar --version)
[[ $tar_version == *'GNU tar'* ]] || die 'GNU tar is required for deterministic packaging'

version=1.14.4-apsp.1
upstream_tag=v1.14.4
upstream_commit=c005f35300ce76427dc285481ec5695f5b179cd2
patched_commit=243a13cd63c275a1567badb2905b731a48545a26
# Server build tags, including the statistics API the Agent reads.
tags=with_quic,with_utls,with_wireguard,with_clash_api,with_v2ray_api,with_acme
output=$2/sing-box/$version
[[ ! -L $output ]] || die 'output version directory must not be a symlink'
mkdir -p "$output"
output=$(cd "$output" && pwd -P)
lock=$output/.build.lock
mkdir "$lock" 2>/dev/null || die "another build owns $lock; remove only after confirming that build has stopped"
scratch=
stage_file=
sums_file=
output_created=0
committed=0
cleanup() {
  if [[ $output_created == 1 && $committed == 0 ]]; then rm -f -- "$output/$artifact"; fi
  [[ -z $stage_file ]] || rm -f -- "$stage_file"
  [[ -z $sums_file ]] || rm -f -- "$sums_file"
  [[ -z $scratch ]] || rm -rf -- "$scratch"
  rmdir -- "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
[[ ! -e $output/$artifact && ! -L $output/$artifact ]] || die "immutable artifact already exists: $output/$artifact"
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
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *]((?:linux-(?:gnu|musl)-)?(?:amd64|arm64))", line)
        if not match or match[2] in expected:
            raise SystemExit("invalid or duplicate SHA256SUMS entry")
        expected[match[2]] = match[1].lower()
for arch in ("amd64", "arm64", "linux-gnu-amd64", "linux-gnu-arm64", "linux-musl-amd64", "linux-musl-arm64"):
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

patch_queue=${APSP_SINGBOX_PATCHES:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)/singbox-runtime}
[[ -f $patch_queue/apply.py && -f $patch_queue/modules.txt ]] || die "missing runtime patch queue: $patch_queue"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/apsp-runtime.XXXXXX")
source_dir=$scratch/source
git -c advice.detachedHead=false clone --depth=1 --branch "$upstream_tag" https://github.com/SagerNet/sing-box.git "$source_dir"
[[ $(git -C "$source_dir" rev-parse HEAD) == "$upstream_commit" ]] || die 'upstream tag resolved to an unexpected commit'
[[ $(python3 "$patch_queue/apply.py" "$source_dir") == "$patched_commit" ]] || die 'patch queue produced an unexpected commit'

cd "$source_dir"
mkdir "$scratch/stage"
for attempt in 1 2 3; do
  if CGO_ENABLED=0 GOOS=linux GOARCH="$arch" GOFLAGS= \
    go build -mod=readonly -v -trimpath -tags "$tags" \
      -ldflags "-X github.com/sagernet/sing-box/constant.Version=$version -s -w -buildid=" \
      -o "$scratch/stage/sing-box" ./cmd/sing-box; then
    break
  fi
  [[ $attempt != 3 ]] || die 'runtime build failed after three attempts'
  printf 'Runtime build attempt %s failed; retrying with the existing module cache.\n' "$attempt" >&2
  sleep 5
done
[[ -z $(git status --porcelain) ]] || die 'upstream source changed during the build'
binary=$scratch/stage/sing-box
chmod 0755 "$binary"
go version -m "$binary" | tee "$scratch/buildinfo"
python3 - "$scratch/buildinfo" "$patch_queue/modules.txt" "$arch" "$tags" "$patched_commit" <<'PY'
import sys

buildinfo, modules, arch, tags, commit = sys.argv[1:]
settings, deps = {}, set()
for line in open(buildinfo).read().splitlines()[1:]:
    fields = line.strip().split("\t")
    if fields[0] == "build":
        key, _, value = fields[1].partition("=")
        settings[key] = value
    elif fields[0] == "dep":
        deps.add(" ".join(fields[1:4]))
    elif fields[0] == "=>":
        raise SystemExit("runtime must not replace modules")
expected = {"GOOS": "linux", "GOARCH": arch, "CGO_ENABLED": "0", "-tags": tags,
            "-trimpath": "true", "vcs.revision": commit, "vcs.modified": "false"}
for key, value in expected.items():
    if settings.get(key) != value:
        raise SystemExit("unexpected build setting: " + key)
pinned = {line.split("#", 1)[0].strip() for line in open(modules)} - {""}
if deps != pinned:
    raise SystemExit("runtime dependencies differ from modules.txt")
PY
program_headers=$(readelf -lW "$binary")
dynamic=$(readelf -dW "$binary")
[[ $program_headers != *INTERP* && $dynamic != *NEEDED* ]] || die 'runtime must be statically linked'
header=$(readelf -h "$binary")
if [[ $arch == amd64 ]]; then
  [[ $header == *'Advanced Micro Devices X86-64'* ]] || die 'built binary has the wrong architecture'
  runtime_version=$(timeout 30 "$binary" version)
  printf '%s\n' "$runtime_version"
  [[ ${runtime_version%%$'\n'*} == "sing-box version $version" && $runtime_version == *"Tags: $tags"* && $runtime_version == *'CGO: disabled'* ]] || die 'native version/tags verification failed'
else
  [[ $header == *AArch64* ]] || die 'built binary has the wrong architecture'
  printf '%s\n' 'Cross-compiled arm64: ELF/build metadata verified; execute version on an arm64 host before deployment.'
fi
stage_file=$(mktemp "$output/.$artifact.XXXXXX")
tar --format=ustar --mtime=@0 --owner=0 --group=0 --numeric-owner --sort=name \
  -cf - -C "$scratch/stage" sing-box | gzip -n -9 > "$stage_file"
chmod 0644 "$stage_file"
ln -- "$stage_file" "$output/$artifact"
output_created=1
sums_file=$(mktemp "$output/.SHA256SUMS.XXXXXX")
(
  cd "$output"
  for candidate in amd64 arm64 linux-gnu-amd64 linux-gnu-arm64 linux-musl-amd64 linux-musl-arm64; do
    if [[ -f $candidate ]]; then sha256sum "$candidate"; fi
  done
) > "$sums_file"
chmod 0644 "$sums_file"
mv -T -- "$sums_file" "$output/SHA256SUMS"
committed=1
printf 'Artifact: %s/%s\n' "$output" "$artifact"
cat "$output/SHA256SUMS"

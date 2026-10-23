#!/usr/bin/env bash
# Build the pinned upstream release without modifying its source or default tags.
set -euo pipefail
umask 022

usage() {
  cat <<'USAGE'
Usage: tools/build-singbox.sh <amd64|arm64> <ARTIFACT_ROOT> [--libc=gnu|--libc=musl]

Run on Linux/amd64 with Go 1.26.8. Both targets use the upstream Chromium
clang/sysroot toolchain. GNU uses glibc >= 2.31; musl is statically linked.

Output: ARTIFACT_ROOT/sing-box/1.14.2/<arch> (a gzip archive with one binary)
        ARTIFACT_ROOT/sing-box/1.14.2/SHA256SUMS
With --libc, the artifact filename is linux-<libc>-<arch>. The legacy default
remains GNU under <arch>. musl retains all upstream default tags.

Debian build prerequisites:
  ca-certificates git curl python3 python3-requests gnupg dirmngr xz-utils
  unzip bzip2 zstd file binutils binutils-aarch64-linux-gnu build-essential
  pkg-config coreutils

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
for tool in go git curl python3 gpg gpgconf gpgv tar gzip sha256sum readelf timeout \
  file xz unzip bzip2 zstd dpkg-deb pkg-config make gcc g++ ar; do
  command -v "$tool" >/dev/null || die "missing build tool: $tool"
done
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || die 'use a Linux/amd64 build host (both targets are supported there)'
export GOTOOLCHAIN=local
[[ $(go env GOVERSION) == go1.26.8 ]] || die 'Go 1.26.8 is required to match the pinned upstream release workflow'
[[ $(go env GOHOSTOS)/$(go env GOHOSTARCH) == linux/amd64 ]] || die 'Go must run natively on Linux/amd64'
tar_version=$(tar --version)
[[ $tar_version == *'GNU tar'* ]] || die 'GNU tar is required for deterministic packaging'
python3 -c 'import requests' || die 'install python3-requests'
if [[ $arch == arm64 ]]; then
  command -v aarch64-linux-gnu-objcopy >/dev/null || die 'install binutils-aarch64-linux-gnu'
fi

version=1.14.2
upstream_commit=af6e64c3b69e6132ebaee0e1a3d24e93903f6709
cronet_commit=0d28acc44093df24b2526dea3d6ffefd6b0a54f0
naive_commit=72a06c9fca0e2d228588c7f3074bf7efff3ff686
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
  if [[ -n $scratch && -d $scratch/gnupg ]]; then
    gpgconf --homedir "$scratch/gnupg" --kill all || true
  fi
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

scratch=$(mktemp -d "${TMPDIR:-/tmp}/apsp-runtime.XXXXXX")
source_dir=$scratch/source
cronet_dir=$scratch/cronet-go
git clone --depth=1 --branch "v$version" https://github.com/SagerNet/sing-box.git "$source_dir"
[[ $(git -C "$source_dir" rev-parse HEAD) == "$upstream_commit" ]] || die 'upstream tag resolved to an unexpected commit'
[[ $(cat "$source_dir/.github/CRONET_GO_VERSION") == "$cronet_commit" ]] || die 'unexpected upstream toolchain revision'
git init "$cronet_dir"
git -C "$cronet_dir" remote add origin https://github.com/SagerNet/cronet-go.git
git -C "$cronet_dir" sparse-checkout set --no-cone '/*' '!/lib'
git -C "$cronet_dir" fetch --depth=1 --filter=blob:none origin "$cronet_commit"
git -C "$cronet_dir" checkout --detach FETCH_HEAD
[[ $(git -C "$cronet_dir" rev-parse HEAD) == "$cronet_commit" ]] || die 'unexpected toolchain checkout'
git -C "$cronet_dir" submodule update --init --recursive --depth=1
[[ $(git -C "$cronet_dir/naiveproxy" rev-parse HEAD) == "$naive_commit" ]] || die 'unexpected Chromium toolchain submodule'

# Isolate upstream keyring generation from the caller's private GnuPG keyring.
mkdir -m 0700 "$scratch/gnupg"
(
  cd "$cronet_dir"
  rm -f naiveproxy/src/build/linux/sysroot_scripts/keyring.gpg
  GNUPGHOME="$scratch/gnupg" GPG_TTY=/dev/null ./naiveproxy/src/build/linux/sysroot_scripts/generate_keyring.sh
  toolchain_args=(--target="linux/$arch")
  [[ $libc != musl ]] || toolchain_args+=(--libc=musl)
  GOFLAGS= go run ./cmd/build-naive "${toolchain_args[@]}" download-toolchain
  GOFLAGS= go run ./cmd/build-naive "${toolchain_args[@]}" env > "$scratch/toolchain.env"
)
caller_cgo_ldflags=${CGO_LDFLAGS:-}
while IFS='=' read -r name value; do
  case "$name" in
    CC|CXX|QEMU_LD_PREFIX) export "$name=$value" ;;
    CGO_LDFLAGS) export "CGO_LDFLAGS=$value${caller_cgo_ldflags:+ $caller_cgo_ldflags}" ;;
    *) die "unexpected toolchain variable: $name" ;;
  esac
done < "$scratch/toolchain.env"
[[ -n ${CC:-} && -n ${CXX:-} ]] || die 'upstream toolchain did not provide its compilers'

cd "$source_dir"
tags="$(cat release/DEFAULT_BUILD_TAGS),with_v2ray_api"
[[ $libc != musl ]] || tags="$tags,with_musl"
shared_ldflags=$(cat release/LDFLAGS)
mkdir "$scratch/stage"
for attempt in 1 2 3; do
  if CGO_ENABLED=1 GOOS=linux GOARCH="$arch" GOFLAGS= \
    go build -mod=readonly -v -trimpath -tags "$tags" \
      -ldflags "-X github.com/sagernet/sing-box/constant.Version=$version $shared_ldflags -s -w -buildid=" \
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
go version -m "$binary"
header=$(readelf -h "$binary")
if [[ $libc == musl ]]; then
  program_headers=$(readelf -lW "$binary")
  dynamic=$(readelf -dW "$binary")
  [[ $program_headers != *INTERP* && $dynamic != *NEEDED* ]] || die 'musl runtime must be statically linked'
  build_metadata=$(go version -m "$binary")
  [[ $build_metadata == *with_musl* ]] || die 'musl runtime lacks its upstream build tag'
fi
if [[ $arch == amd64 ]]; then
  [[ $header == *'Advanced Micro Devices X86-64'* ]] || die 'built binary has the wrong architecture'
  runtime_version=$(timeout 30 "$binary" version)
  printf '%s\n' "$runtime_version"
  [[ ${runtime_version%%$'\n'*} == "sing-box version $version" && $runtime_version == *with_v2ray_api* ]] || die 'native version/tags verification failed'
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

#!/usr/bin/env python3
"""Prepare reproducible Linux amd64/arm64 component inputs and Corresponding Source.

Thin supply glue around existing repository builders:
  - tools/build-nodequality.sh + crates/observability/upstream/nodequality/source-helper.py
  - tools/network_workbench_artifact.py + tools/artifact_manifest.py
  - tools/ipquality-rootfs.py + tools/build-ipquality.py
  - tools/singbox-runtime/ + tools/build-singbox.sh

All embedded archives are internal build inputs for the self-contained Agent
(ADR 0141) and never expose standalone host installation.
"""
import argparse
import errno
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

sys.dont_write_bytecode = True

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_DIR = REPO_ROOT / "tools"
if str(TOOLS_DIR) not in sys.path:
    sys.path.insert(0, str(TOOLS_DIR))

import artifact_manifest  # noqa: E402
import network_workbench_artifact as wb_packer  # noqa: E402

DEFAULT_LOCK_FILE = (
    REPO_ROOT / "crates/observability/upstream/agent-linux-tools-lock.json"
)
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SHA512_RE = re.compile(r"^[0-9a-f]{128}$")
SEGMENT_RE = re.compile(r"^[A-Za-z0-9.+_-]{1,128}$")

ELF_MACHINE = {"amd64": 62, "arm64": 183}
CLANG_TARGETS = {
    "amd64": "x86_64-alpine-linux-musl",
    "arm64": "aarch64-alpine-linux-musl",
}
GNU_GCC_CANDIDATES = {
    "amd64": ("x86_64-linux-gnu-gcc", "gcc"),
    "arm64": ("aarch64-linux-gnu-gcc", "gcc"),
}
FALLBACK_GO_IMAGE = "golang:1.27.3-bookworm"

PYTHON_PRUNE_DIRS = frozenset(
    {
        "__pycache__",
        "test",
        "tests",
        "idlelib",
        "tkinter",
        "turtledemo",
        "ensurepip",
        "pydoc_data",
        "venv",
        "lib2to3",
    }
)

WORKBENCH_DYNAMIC_BINS = {
    "ping": ("bin/ping", "usr/bin/ping", "libexec/ping"),
    "mtr": ("usr/sbin/mtr", "usr/bin/mtr", "libexec/mtr"),
    "mtr-packet": ("usr/sbin/mtr-packet", "usr/bin/mtr-packet", "libexec/mtr-packet"),
    "iperf3": ("usr/bin/iperf3", "bin/iperf3", "libexec/iperf3"),
    "sysbench": ("usr/bin/sysbench", "bin/sysbench", "libexec/sysbench"),
    "fio": ("usr/bin/fio", "bin/fio", "libexec/fio"),
    "stress-ng": ("usr/bin/stress-ng", "bin/stress-ng", "libexec/stress-ng"),
    "smartctl": ("usr/sbin/smartctl", "usr/bin/smartctl", "libexec/smartctl"),
    "traceroute": ("usr/bin/traceroute", "bin/traceroute", "libexec/traceroute"),
}
NODEQUALITY_CORE_APPLETS = ("uname", "mkdir", "rm", "rmdir", "cat", "chmod", "sleep")

TRAMPOLINE_C = r"""#define _GNU_SOURCE
#include <limits.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifndef APSP_SELF_DEPTH
#define APSP_SELF_DEPTH 2
#endif

static void fail(const char *msg) {
    (void)write(2, msg, strlen(msg));
    _exit(127);
}

static int join2(char *out, size_t cap, const char *a, const char *b) {
    size_t la = strlen(a), lb = strlen(b);
    if (la + lb + 1 > cap) return -1;
    memcpy(out, a, la);
    memcpy(out + la, b, lb + 1);
    return 0;
}

static int join3(char *out, size_t cap, const char *a, const char *b, const char *c) {
    size_t la = strlen(a), lb = strlen(b), lc = strlen(c);
    if (la + lb + lc + 1 > cap) return -1;
    memcpy(out, a, la);
    memcpy(out + la, b, lb);
    memcpy(out + la + lb, c, lc + 1);
    return 0;
}

int main(int argc, char **argv) {
    char self_path[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", self_path, sizeof(self_path) - 1);
    if (n <= 0 || (size_t)n >= sizeof(self_path) - 1)
        fail("apsp-trampoline: readlink /proc/self/exe failed\n");
    self_path[n] = '\0';

    char root[PATH_MAX];
    memcpy(root, self_path, (size_t)n + 1);
    for (int i = 0; i < APSP_SELF_DEPTH; i++) {
        char *slash = strrchr(root, '/');
        if (!slash || slash == root) fail("apsp-trampoline: invalid bundle layout\n");
        *slash = '\0';
    }

    char ld_path[PATH_MAX], lib_dir[PATH_MAX], target_path[PATH_MAX];
    if (join3(ld_path, sizeof(ld_path), root, "/", APSP_LD_REL) != 0 ||
        join2(lib_dir, sizeof(lib_dir), root, "/lib") != 0 ||
        join3(target_path, sizeof(target_path), root, "/", APSP_TARGET_REL) != 0)
        fail("apsp-trampoline: path overflow\n");

    const char *prev_ld = getenv("LD_LIBRARY_PATH");
    if (prev_ld && prev_ld[0] != '\0') {
        char merged[PATH_MAX * 2];
        if (join3(merged, sizeof(merged), lib_dir, ":", prev_ld) == 0)
            setenv("LD_LIBRARY_PATH", merged, 1);
    } else {
        setenv("LD_LIBRARY_PATH", lib_dir, 1);
    }

#ifdef APSP_SET_PYTHONHOME
    char py_home[PATH_MAX];
    if (join2(py_home, sizeof(py_home), root, "/python") != 0)
        fail("apsp-trampoline: python home overflow\n");
    setenv("PYTHONHOME", py_home, 1);
    setenv("PYTHONDONTWRITEBYTECODE", "1", 1);
#endif

#ifdef APSP_SET_MTR_PACKET
    if (!getenv("MTR_PACKET")) {
        char mtr_pkt[PATH_MAX];
        if (join2(mtr_pkt, sizeof(mtr_pkt), root, "/bin/mtr-packet") == 0)
            setenv("MTR_PACKET", mtr_pkt, 1);
    }
#endif

    char cert_file[PATH_MAX];
    if (join2(cert_file, sizeof(cert_file), root, "/certs/ca-certificates.crt") == 0) {
        if (!getenv("SSL_CERT_FILE")) setenv("SSL_CERT_FILE", cert_file, 1);
        if (!getenv("CURL_CA_BUNDLE")) setenv("CURL_CA_BUNDLE", cert_file, 1);
    }
    char terminfo_dir[PATH_MAX];
    if (!getenv("TERMINFO") &&
        join2(terminfo_dir, sizeof(terminfo_dir), root, "/share/terminfo") == 0)
        setenv("TERMINFO", terminfo_dir, 1);

    char **new_argv = (char **)calloc((size_t)argc + 6, sizeof(char *));
    if (!new_argv) fail("apsp-trampoline: allocation failed\n");
    new_argv[0] = ld_path;
    new_argv[1] = "--library-path";
    new_argv[2] = lib_dir;
    new_argv[3] = "--argv0";
    new_argv[4] = self_path;
    new_argv[5] = target_path;
    for (int i = 1; i < argc; i++) new_argv[i + 5] = argv[i];
    new_argv[argc + 5] = NULL;

    execv(ld_path, new_argv);
    fail("apsp-trampoline: execv failed\n");
    return 127;
}
"""


def die(message: str) -> "None":
    raise SystemExit(f"Error: {message}")


def ensure(condition: bool, message: str) -> None:
    if not condition:
        die(message)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_digest(path: Path, algo: str = "sha256") -> str:
    h = hashlib.new(algo)
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()


def host_linux_arch() -> str:
    machine = platform.machine().lower()
    if machine in ("x86_64", "amd64"):
        return "amd64"
    if machine in ("aarch64", "arm64"):
        return "arm64"
    return machine


def check_bundled_path(relpath: str) -> str:
    normalized = wb_packer.safe_relative_path(relpath)
    ensure(
        all(SEGMENT_RE.fullmatch(part) for part in normalized.split("/")),
        f"bundled path contains disallowed characters: {relpath!r}",
    )
    return normalized


def verify_static_elf64(data: bytes, arch: str, label: str) -> None:
    ensure(
        len(data) >= 64 and data[:6] == b"\x7fELF\x02\x01",
        f"{label} is not a 64-bit little-endian ELF binary",
    )
    machine = int.from_bytes(data[18:20], "little")
    ensure(
        machine == ELF_MACHINE[arch],
        f"{label} ELF e_machine={machine} does not match {arch}",
    )
    phoff = int.from_bytes(data[32:40], "little")
    phentsize = int.from_bytes(data[54:56], "little")
    phnum = int.from_bytes(data[56:58], "little")
    ensure(
        phentsize >= 56 and phoff + phentsize * phnum <= len(data),
        f"{label} has invalid ELF64 program headers",
    )
    for idx in range(phnum):
        offset = phoff + idx * phentsize
        p_type = int.from_bytes(data[offset : offset + 4], "little")
        ensure(p_type != 3, f"{label} contains PT_INTERP; expected static ELF")


def pack_deterministic_tar_gz(entries: dict[str, tuple[bytes, int]]) -> bytes:
    raw_tar = io.BytesIO()
    with tarfile.open(fileobj=raw_tar, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name in sorted(entries):
            content, mode = entries[name]
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = mode
            info.mtime = info.uid = info.gid = 0
            info.uname = info.gname = ""
            archive.addfile(info, io.BytesIO(content))
    return gzip.compress(raw_tar.getvalue(), compresslevel=9, mtime=0)


def write_deterministic_tar_gz(
    path: Path, entries: dict[str, tuple[bytes | Path, int]], mode: int = 0o644
) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        dir=path.parent, prefix=f".{path.name}.", delete=False
    ) as tmp:
        tmp_path = Path(tmp.name)
        try:
            with gzip.GzipFile(
                filename="", mode="wb", fileobj=tmp, compresslevel=9, mtime=0
            ) as gz_out:
                with tarfile.open(
                    fileobj=gz_out, mode="w", format=tarfile.PAX_FORMAT
                ) as archive:
                    for name in sorted(entries):
                        content, entry_mode = entries[name]
                        info = tarfile.TarInfo(name)
                        info.mode = entry_mode
                        info.mtime = info.uid = info.gid = 0
                        info.uname = info.gname = ""
                        if isinstance(content, Path):
                            info.size = content.stat().st_size
                            with content.open("rb") as src_f:
                                archive.addfile(info, src_f)
                        else:
                            info.size = len(content)
                            archive.addfile(info, io.BytesIO(content))
            tmp.flush()
            os.fsync(tmp.fileno())
        except BaseException:
            tmp_path.unlink(missing_ok=True)
            raise
    os.chmod(tmp_path, mode)
    os.replace(tmp_path, path)
    return file_digest(path, "sha256")


def publish_idempotent(directory: Path, name: str, payload: bytes) -> Path:
    target = directory / name
    expected_sha = sha256_bytes(payload)
    if target.exists() or target.is_symlink():
        ensure(
            target.is_file() and not target.is_symlink(),
            f"existing artifact target is not an ordinary file: {target}",
        )
        actual_sha = file_digest(target, "sha256")
        ensure(
            actual_sha == expected_sha,
            f"immutable artifact {target} already exists with different sha256 ({actual_sha} != {expected_sha})",
        )
        sums = directory / "SHA256SUMS"
        ensure(
            sums.is_file()
            and not sums.is_symlink()
            and f"{expected_sha}  {name}" in sums.read_text(encoding="utf-8").splitlines(),
            f"existing artifact {target} lacks matching SHA256SUMS entry",
        )
        return target
    return artifact_manifest.publish(directory, name, payload)


def upstream_cache_candidates(cache_dir: Path | None, filename: str) -> list[Path]:
    if cache_dir is None:
        return []
    return [
        cache_dir / "upstream" / filename,
        cache_dir / filename,
        cache_dir.parent / "upstream-sources" / filename,
        cache_dir.parent / "scratch/upstream-check" / filename,
    ]


def atomic_write(path: Path, payload: bytes, mode: int = 0o644) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        dir=path.parent, prefix=f".{path.name}.", delete=False
    ) as tmp:
        tmp.write(payload)
        tmp.flush()
        os.fsync(tmp.fileno())
        tmp_path = Path(tmp.name)
    os.chmod(tmp_path, mode)
    os.replace(tmp_path, path)
    return sha256_bytes(payload)


def https_fetch(
    url: str,
    dest: Path,
    *,
    size: int | None = None,
    sha256: str | None = None,
    sha512: str | None = None,
    max_bytes: int = 256 * 1024 * 1024,
) -> Path:
    ensure(urllib.parse.urlparse(url).scheme == "https", f"HTTPS required: {url}")
    dest.parent.mkdir(parents=True, exist_ok=True)
    last_exc: BaseException | None = None
    for attempt in range(3):
        req = urllib.request.Request(url, headers={"User-Agent": "apsp-inputs/1.0"})
        h256, h512, total = hashlib.sha256(), hashlib.sha512(), 0
        with tempfile.NamedTemporaryFile(
            dir=dest.parent, prefix=f".{dest.name}.", delete=False
        ) as tmp:
            tmp_path = Path(tmp.name)
            try:
                with urllib.request.urlopen(req, timeout=120) as resp:
                    ensure(resp.status == 200, f"HTTP {resp.status} for {url}")
                    while chunk := resp.read(256 * 1024):
                        total += len(chunk)
                        ensure(total <= max_bytes, f"download exceeded ceiling: {url}")
                        if size is not None:
                            ensure(total <= size, f"download exceeded expected size: {url}")
                        h256.update(chunk)
                        h512.update(chunk)
                        tmp.write(chunk)
                tmp.flush()
                os.fsync(tmp.fileno())
                last_exc = None
                break
            except urllib.error.HTTPError as exc:
                tmp_path.unlink(missing_ok=True)
                if exc.code in (408, 429, 500, 502, 503, 504) and attempt < 2:
                    last_exc = exc
                    time.sleep(2 * (attempt + 1))
                    continue
                raise
            except (OSError, TimeoutError) as exc:
                tmp_path.unlink(missing_ok=True)
                if attempt < 2:
                    last_exc = exc
                    time.sleep(2 * (attempt + 1))
                    continue
                raise
            except BaseException:
                tmp_path.unlink(missing_ok=True)
                raise
    if last_exc is not None:
        raise last_exc
    if (
        (size is not None and total != size)
        or (sha256 is not None and h256.hexdigest() != sha256)
        or (sha512 is not None and h512.hexdigest() != sha512)
    ):
        tmp_path.unlink(missing_ok=True)
        die(f"digest or length mismatch for {url}")
    os.chmod(tmp_path, 0o644)
    os.replace(tmp_path, dest)
    return dest


def obtain_locked(
    url: str,
    dest: Path,
    cache_candidates: list[Path],
    *,
    size: int | None = None,
    sha256: str | None = None,
    sha512: str | None = None,
) -> Path:
    for cand in [dest, *cache_candidates]:
        if not cand.is_file() or cand.is_symlink():
            continue
        if size is not None and cand.stat().st_size != size:
            continue
        if sha256 is not None and file_digest(cand, "sha256") != sha256:
            continue
        if sha512 is not None and file_digest(cand, "sha512") != sha512:
            continue
        if cand != dest:
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(cand, dest)
        return dest
    return https_fetch(url, dest, size=size, sha256=sha256, sha512=sha512)


def extract_apk(apk_path: Path, rootfs_dir: Path) -> None:
    rootfs_dir.mkdir(parents=True, exist_ok=True)
    root_resolved = rootfs_dir.resolve()
    with tarfile.open(apk_path, mode="r:*") as archive:
        for member in archive.getmembers():
            raw = member.name.lstrip("./")
            if not raw or raw.startswith("."):
                continue
            parts = Path(raw).parts
            ensure(
                all(p not in ("", ".", "..") for p in parts),
                f"unsafe APK member {member.name!r}",
            )
            dest = rootfs_dir.joinpath(*parts)
            dest.parent.mkdir(parents=True, exist_ok=True)
            ensure(
                dest.parent.resolve().is_relative_to(root_resolved),
                f"APK parent escaped rootfs: {member.name}",
            )
            if member.isdir():
                if dest.is_symlink():
                    dest.unlink()
                dest.mkdir(parents=True, exist_ok=True)
            elif member.issym():
                if dest.exists() or dest.is_symlink():
                    shutil.rmtree(dest) if (dest.is_dir() and not dest.is_symlink()) else dest.unlink()
                os.symlink(member.linkname, dest)
            elif member.islnk():
                target = rootfs_dir.joinpath(*Path(member.linkname.lstrip("./")).parts)
                ensure(
                    target.resolve().is_relative_to(root_resolved) and target.is_file(),
                    f"invalid APK hardlink {member.name}",
                )
                if dest.exists() or dest.is_symlink():
                    dest.unlink()
                shutil.copyfile(target, dest)
                os.chmod(dest, 0o755 if (member.mode & 0o111) else 0o644)
            elif member.isfile():
                stream = archive.extractfile(member)
                ensure(stream is not None, f"empty APK member {member.name}")
                if dest.exists() or dest.is_symlink():
                    dest.unlink()
                dest.write_bytes(stream.read())
                os.chmod(dest, 0o755 if (member.mode & 0o111) else 0o644)


def resolve_in_rootfs(rootfs_dir: Path, rel_path: str) -> Path:
    root_resolved = rootfs_dir.resolve()
    cur = rootfs_dir / rel_path.lstrip("/")
    for _ in range(16):
        if not cur.is_symlink():
            ensure(cur.resolve().is_relative_to(root_resolved), f"escaped rootfs: {rel_path}")
            return cur
        link_val = os.readlink(cur)
        cur = (rootfs_dir / link_val.lstrip("/")) if link_val.startswith("/") else (cur.parent / link_val)
    die(f"symlink loop in {rel_path}")


def stage_alpine_runtime_closure(
    rootfs_dir: Path, bundle_dir: Path, ld_musl_name: str, py_ver: str
) -> Path:
    lib_dir = bundle_dir / "lib"
    lib_dir.mkdir(parents=True, exist_ok=True)
    seen: set[str] = set()
    for sub in ("lib", "usr/lib"):
        scan = rootfs_dir / sub
        if not scan.is_dir():
            continue
        for item in sorted(scan.iterdir(), key=lambda p: p.name):
            name = item.name
            if name in seen or not (name == ld_musl_name or name.endswith(".so") or ".so." in name):
                continue
            target = resolve_in_rootfs(rootfs_dir, f"{sub}/{name}")
            if target.is_file():
                check_bundled_path(f"lib/{name}")
                out = lib_dir / name
                out.write_bytes(target.read_bytes())
                os.chmod(out, 0o755 if name == ld_musl_name else 0o644)
                seen.add(name)
    ensure((lib_dir / ld_musl_name).is_file(), f"missing {ld_musl_name}")

    src_std = rootfs_dir / "usr/lib" / f"python{py_ver}"
    ensure(src_std.is_dir(), f"missing Python stdlib: {src_std}")
    for cur_root, dirs, files in os.walk(src_std, followlinks=False):
        rel_dir = Path(cur_root).relative_to(src_std)
        dirs[:] = [
            d for d in sorted(dirs)
            if d not in PYTHON_PRUNE_DIRS and not d.startswith(f"config-{py_ver}")
        ]
        for fname in sorted(files):
            if fname.endswith((".pyc", ".pyo", ".a")):
                continue
            item = Path(cur_root) / fname
            resolved = (
                resolve_in_rootfs(rootfs_dir, str(item.relative_to(rootfs_dir)))
                if item.is_symlink()
                else item
            )
            if not resolved.is_file():
                continue
            data = resolved.read_bytes()
            exec_bit = bool(resolved.stat().st_mode & 0o111)
            if not data and exec_bit:
                continue
            rel_out = (Path("python/lib") / f"python{py_ver}" / rel_dir / fname).as_posix()
            check_bundled_path(rel_out)
            dst = bundle_dir / rel_out
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(data)
            os.chmod(dst, 0o755 if (exec_bit or fname.endswith(".so")) else 0o644)

    for ti_sub in ("etc/terminfo", "usr/share/terminfo"):
        src_ti = rootfs_dir / ti_sub
        if not src_ti.is_dir() or src_ti.is_symlink():
            continue
        for cur_root, dirs, files in os.walk(src_ti, followlinks=False):
            dirs.sort()
            rel_dir = Path(cur_root).relative_to(src_ti)
            for fname in sorted(files):
                item = Path(cur_root) / fname
                resolved = (
                    resolve_in_rootfs(rootfs_dir, str(item.relative_to(rootfs_dir)))
                    if item.is_symlink()
                    else item
                )
                if resolved.is_file() and resolved.stat().st_size > 0:
                    rel_out = (Path("share/terminfo") / rel_dir / fname).as_posix()
                    check_bundled_path(rel_out)
                    dst = bundle_dir / rel_out
                    dst.parent.mkdir(parents=True, exist_ok=True)
                    dst.write_bytes(resolved.read_bytes())
                    os.chmod(dst, 0o644)

    ca_src = resolve_in_rootfs(rootfs_dir, "etc/ssl/certs/ca-certificates.crt")
    ensure(ca_src.is_file(), "missing ca-certificates.crt")
    ca_dst = bundle_dir / "certs/ca-certificates.crt"
    ca_dst.parent.mkdir(parents=True, exist_ok=True)
    ca_dst.write_bytes(ca_src.read_bytes())
    os.chmod(ca_dst, 0o644)
    return ca_dst


def compile_trampoline(
    arch: str,
    musl_sysroot: Path,
    source_file: Path,
    output_file: Path,
    *,
    ld_rel: str,
    target_rel: str,
    self_depth: int = 2,
    set_pythonhome: bool = False,
    set_mtr_packet: bool = False,
) -> bytes:
    output_file.parent.mkdir(parents=True, exist_ok=True)
    defines = [
        f'-DAPSP_LD_REL="{ld_rel}"',
        f'-DAPSP_TARGET_REL="{target_rel}"',
        f"-DAPSP_SELF_DEPTH={self_depth}",
    ]
    if set_pythonhome:
        defines.append("-DAPSP_SET_PYTHONHOME=1")
    if set_mtr_packet:
        defines.append("-DAPSP_SET_MTR_PACKET=1")

    crt1 = str(musl_sysroot / "usr/lib/crt1.o")
    crti = str(musl_sysroot / "usr/lib/crti.o")
    crtn = str(musl_sysroot / "usr/lib/crtn.o")
    libdir = f"-L{musl_sysroot / 'usr/lib'}"
    common_flags = ["-static", "-Os", "-fno-stack-protector", "-Wl,--build-id=none", "-s", *defines]
    attempts: list[list[str]] = []

    if host_linux_arch() == arch and shutil.which("musl-gcc"):
        attempts.append(["musl-gcc", *common_flags, str(source_file), "-o", str(output_file)])
    if shutil.which("clang"):
        lld = ["-fuse-ld=lld"] if shutil.which("ld.lld") else []
        attempts.append(
            [
                "clang",
                f"--target={CLANG_TARGETS[arch]}",
                f"--sysroot={musl_sysroot}",
                *lld,
                "-nostdlib",
                "-fno-pie",
                "-no-pie",
                *common_flags,
                crt1,
                crti,
                str(source_file),
                libdir,
                "-lc",
                crtn,
                "-o",
                str(output_file),
            ]
        )
    for gcc_name in GNU_GCC_CANDIDATES[arch]:
        if gcc_name == "gcc" and host_linux_arch() != arch:
            continue
        gcc_bin = shutil.which(gcc_name)
        if not gcc_bin:
            continue
        lgcc = subprocess.run([gcc_bin, "-print-libgcc-file-name"], capture_output=True, text=True)
        extra = [lgcc.stdout.strip()] if (lgcc.returncode == 0 and Path(lgcc.stdout.strip()).is_file()) else []
        attempts.append(
            [
                gcc_bin,
                "-nostdinc",
                "-isystem",
                str(musl_sysroot / "usr/include"),
                "-nostdlib",
                "-fno-pie",
                "-no-pie",
                *common_flags,
                crt1,
                crti,
                str(source_file),
                libdir,
                "-lc",
                *extra,
                crtn,
                "-o",
                str(output_file),
            ]
        )

    errors = []
    for cmd in attempts:
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode == 0 and output_file.is_file():
            os.chmod(output_file, 0o755)
            data = output_file.read_bytes()
            verify_static_elf64(data, arch, output_file.name)
            return data
        errors.append(f"{cmd[0]}: {proc.stderr.strip()}")

    if host_linux_arch() == arch and shutil.which("docker"):
        apk_arch = "x86_64" if arch == "amd64" else "aarch64"
        img_cands = [
            os.environ.get("APSP_AGENT_BUILDER_IMAGE", "").strip(),
            f"apsp-local-agent-builder:20261118-{apk_arch}",
            FALLBACK_GO_IMAGE,
        ]
        q_flags = " ".join(shlex.quote(f) for f in common_flags)
        for img in [c for c in img_cands if c]:
            insp = subprocess.run(["docker", "image", "inspect", img], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if insp.returncode != 0:
                continue
            script = (
                f"if command -v musl-gcc >/dev/null 2>&1; then "
                f"exec musl-gcc {q_flags} /src/{shlex.quote(source_file.name)} -o /out/{shlex.quote(output_file.name)}; "
                f"fi; "
                f"lgcc=$(gcc -print-libgcc-file-name); "
                f"exec gcc -nostdinc -isystem /sysroot/usr/include -nostdlib -fno-pie -no-pie {q_flags} "
                f"/sysroot/usr/lib/crt1.o /sysroot/usr/lib/crti.o /src/{shlex.quote(source_file.name)} "
                f"-L/sysroot/usr/lib -lc \"$lgcc\" /sysroot/usr/lib/crtn.o -o /out/{shlex.quote(output_file.name)}"
            )
            proc = subprocess.run(
                [
                    "docker", "run", "--rm", "--entrypoint", "sh",
                    "-u", f"{os.geteuid()}:{os.getegid()}",
                    "-v", f"{musl_sysroot.resolve()}:/sysroot:ro",
                    "-v", f"{source_file.parent.resolve()}:/src:ro",
                    "-v", f"{output_file.parent.resolve()}:/out",
                    img, "-eu", "-c", script,
                ],
                capture_output=True,
                text=True,
            )
            if proc.returncode == 0 and output_file.is_file():
                os.chmod(output_file, 0o755)
                data = output_file.read_bytes()
                verify_static_elf64(data, arch, output_file.name)
                return data
            errors.append(f"docker({img}): {proc.stderr.strip()}")

    die(f"unable to compile static musl {arch} launcher ({'; '.join(errors)})")


def bounded_go_flags() -> tuple[str, str]:
    tokens = [t for t in os.environ.get("GOFLAGS", "").split() if t]
    if not any(t.startswith("-buildvcs") for t in tokens):
        tokens.append("-buildvcs=false")
    if not any(t == "-p" or t.startswith("-p=") for t in tokens):
        tokens.append("-p=1")
    gomaxprocs = os.environ.get("GOMAXPROCS", "").strip() or "2"
    return " ".join(tokens), gomaxprocs


def restore_cached_nexttrace_vendor(
    src_dir: Path, version: str, cache_dir: Path | None
) -> bool:
    if cache_dir is None:
        return False
    fname = f"NTrace-core-{version}-vendored.tar.gz"
    cands = [
        *upstream_cache_candidates(cache_dir, fname),
        cache_dir.parent / "scratch/upstream-check/nt-out" / fname,
    ]
    prefix = f"NTrace-core-{version}/vendor/"
    src_resolved = src_dir.resolve()
    for cand in cands:
        if not cand.is_file() or cand.is_symlink():
            continue
        with tarfile.open(cand, mode="r:gz") as tf:
            members = [m for m in tf.getmembers() if m.name.startswith(prefix)]
            if not members:
                continue
            for m in members:
                rel = m.name[len(f"NTrace-core-{version}/") :]
                parts = Path(rel).parts
                ensure(
                    bool(parts)
                    and all(p not in ("", ".", "..") for p in parts)
                    and not m.issym()
                    and not m.islnk(),
                    f"unsafe cached vendor member: {m.name}",
                )
                dest = src_dir.joinpath(*parts)
                ensure(dest.resolve().is_relative_to(src_resolved), f"vendor path escape: {m.name}")
                if m.isdir():
                    dest.mkdir(parents=True, exist_ok=True)
                elif m.isfile():
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    stream = tf.extractfile(m)
                    ensure(stream is not None, f"empty vendor member {m.name}")
                    dest.write_bytes(stream.read())
                    os.chmod(dest, 0o755 if (m.mode & 0o111) else 0o644)
            return (src_dir / "vendor/modules.txt").is_file()
    return False


def build_nexttrace(
    arch: str,
    ntrace_tarball: Path,
    ntrace_lock: dict,
    work_dir: Path,
    cache_dir: Path | None = None,
) -> tuple[bytes, Path]:
    nt_work = work_dir / "nexttrace-build"
    shutil.rmtree(nt_work, ignore_errors=True)
    nt_work.mkdir(parents=True, exist_ok=True)
    root_resolved = nt_work.resolve()
    with tarfile.open(ntrace_tarball, mode="r:gz") as archive:
        for m in archive.getmembers():
            parts = Path(m.name.lstrip("./")).parts
            if not parts:
                continue
            ensure(
                all(p not in ("", ".", "..") for p in parts) and not m.issym() and not m.islnk(),
                f"unsafe nexttrace member: {m.name}",
            )
            dest = nt_work.joinpath(*parts)
            ensure(dest.resolve().is_relative_to(root_resolved), f"path escape: {m.name}")
            if m.isdir():
                dest.mkdir(parents=True, exist_ok=True)
            elif m.isfile():
                dest.parent.mkdir(parents=True, exist_ok=True)
                stream = archive.extractfile(m)
                ensure(stream is not None, f"empty member {m.name}")
                dest.write_bytes(stream.read())
                os.chmod(dest, 0o755 if (m.mode & 0o111) else 0o644)

    subdirs = [d for d in nt_work.iterdir() if d.is_dir()]
    ensure(len(subdirs) == 1, "unexpected NTrace-core tarball layout")
    src_dir = subdirs[0]
    ldflags = f"-s -w -buildid= -X {ntrace_lock['version_package']}.Version={ntrace_lock['tag']}"
    out_bin = nt_work / f"nexttrace-{arch}"
    goflags, gomaxprocs = bounded_go_flags()

    min_go = (1, 27, 3)
    gm_path = src_dir / "go.mod"
    if gm_path.is_file():
        gm_match = re.search(r"(?m)^go\s+(\d+)\.(\d+)(?:\.(\d+))?\s*$", gm_path.read_text(encoding="utf-8", errors="replace"))
        if gm_match:
            min_go = (int(gm_match.group(1)), int(gm_match.group(2)), int(gm_match.group(3) or 0))

    def go_meets_min(cand_bin: str) -> bool:
        proc = subprocess.run([cand_bin, "version"], capture_output=True, text=True)
        if proc.returncode != 0:
            return False
        vm = re.search(r"go(\d+)\.(\d+)(?:\.(\d+))?", proc.stdout)
        if not vm:
            return False
        return (int(vm.group(1)), int(vm.group(2)), int(vm.group(3) or 0)) >= min_go

    home = Path.home()
    go_cands: list[str] = []
    if found_go := shutil.which("go"):
        go_cands.append(found_go)
    for cand in (
        home / ".local/go/bin/go",
        home / "go-sdk/bin/go",
        Path("/usr/local/go/bin/go"),
        *sorted(Path("/opt/hostedtoolcache/go").glob("*/*/bin/go"), reverse=True),
    ):
        if cand.is_file() and os.access(cand, os.X_OK):
            go_cands.append(str(cand))
    go_bin = next((c for c in go_cands if go_meets_min(c)), None)
    if not go_bin and not shutil.which("docker") and go_cands:
        go_bin = go_cands[0]

    if go_bin:
        env = {
            **os.environ,
            "CGO_ENABLED": "0",
            "GOFLAGS": goflags,
            "GOMAXPROCS": gomaxprocs,
        }
        if not (src_dir / "vendor").is_dir():
            try:
                subprocess.run([go_bin, "mod", "vendor"], cwd=src_dir, env=env, check=True)
            except subprocess.CalledProcessError:
                if not restore_cached_nexttrace_vendor(src_dir, ntrace_lock["version"], cache_dir):
                    raise
        subprocess.run(
            [
                go_bin, "build", "-mod=vendor", "-trimpath",
                "-tags", ntrace_lock["build_tags"], "-ldflags", ldflags,
                "-o", str(out_bin), ".",
            ],
            cwd=src_dir,
            env={**env, "GOOS": "linux", "GOARCH": arch},
            check=True,
        )
    elif shutil.which("docker"):
        go_image = os.environ.get("APSP_GO_IMAGE", "").strip() or FALLBACK_GO_IMAGE
        docker_base = [
            "docker", "run", "--rm", "--entrypoint", "sh",
            "-u", f"{os.geteuid()}:{os.getegid()}",
            "-e", "HOME=/tmp", "-e", "GOPATH=/tmp/go", "-e", "GOCACHE=/tmp/go-cache",
            "-e", "CGO_ENABLED=0", "-e", f"GOFLAGS={goflags}", "-e", f"GOMAXPROCS={gomaxprocs}",
            "-e", "PATH=/usr/local/go/bin:/usr/bin:/bin",
            "-v", f"{nt_work}:/work", "-w", f"/work/{src_dir.name}",
            go_image, "-eu", "-c",
        ]
        if not (src_dir / "vendor").is_dir():
            try:
                subprocess.run([*docker_base, "go mod vendor"], check=True)
            except subprocess.CalledProcessError:
                if not restore_cached_nexttrace_vendor(src_dir, ntrace_lock["version"], cache_dir):
                    raise
        subprocess.run(
            [
                *docker_base,
                f"GOOS=linux GOARCH={arch} go build -mod=vendor -trimpath "
                f"-tags {shlex.quote(ntrace_lock['build_tags'])} "
                f"-ldflags {shlex.quote(ldflags)} -o /work/{shlex.quote(out_bin.name)} .",
            ],
            check=True,
        )
    else:
        die("Go toolchain or Docker required to build nexttrace")

    bin_bytes = out_bin.read_bytes()
    verify_static_elf64(bin_bytes, arch, "nexttrace")

    prefix = f"NTrace-core-{ntrace_lock['version']}"
    vendored: dict[str, tuple[bytes | Path, int]] = {}
    for cur_root, dirs, files in os.walk(src_dir, followlinks=False):
        dirs.sort()
        rel_dir = Path(cur_root).relative_to(src_dir)
        for fname in sorted(files):
            fpath = Path(cur_root) / fname
            if fpath.is_file() and not fpath.is_symlink():
                mode = 0o755 if (fpath.stat().st_mode & 0o111) else 0o644
                vendored[(Path(prefix) / rel_dir / fname).as_posix()] = (fpath, mode)
    vendored_tar = nt_work / f"NTrace-core-{ntrace_lock['version']}-vendored.tar.gz"
    write_deterministic_tar_gz(vendored_tar, vendored)
    return bin_bytes, vendored_tar


def expand_apkbuild_vars(template: str, variables: dict[str, str]) -> str:
    def repl(match: re.Match[str]) -> str:
        expr = match.group(1)
        for op in ("//", "/", "%%", "%", "##", "#"):
            if op in expr:
                var, rest = expr.split(op, 1)
                val = variables.get(var, "")
                if op in ("//", "/"):
                    old, _, new = rest.partition("/")
                    return val.replace(old, new) if op == "//" else val.replace(old, new, 1)
                if op in ("%%", "%") and rest.endswith("*"):
                    idx = val.find(rest[:-1]) if op == "%%" else val.rfind(rest[:-1])
                    return val[:idx] if idx != -1 else val
                if op in ("##", "#") and rest.startswith("*"):
                    idx = val.rfind(rest[1:]) if op == "##" else val.find(rest[1:])
                    return val[idx + len(rest) - 1 :] if idx != -1 else val
                return val.removesuffix(rest) if "%" in op else val.removeprefix(rest)
        return variables.get(expr, "")

    out = re.sub(r"\$\{([^{}]+)\}", repl, template)
    return re.sub(r"\$([A-Za-z_][A-Za-z0-9_]*)", lambda m: variables.get(m.group(1), ""), out)


def collect_apk_corresponding_sources(
    origins_needed: set[str], alpine_lock: dict, work_dir: Path, cache_dir: Path | None
) -> dict[str, tuple[bytes | Path, int]]:
    aports_base = alpine_lock["aports_raw_base"]
    mirrors = alpine_lock["distfiles_mirrors"]
    origins_meta = alpine_lock["origins"]
    apkbuild_dir = work_dir / "apkbuilds"
    distfiles_dir = work_dir / "distfiles"
    entries: dict[str, tuple[bytes | Path, int]] = {}
    manifest_rows = []

    for origin in sorted(origins_needed):
        meta = origins_meta[origin]
        cands = (
            [cache_dir / "aports-sources" / f"{origin}.APKBUILD", cache_dir / "apkbuilds" / f"{origin}.APKBUILD"]
            if cache_dir else []
        )
        apkbuild_path = obtain_locked(
            meta["apkbuild_url"],
            apkbuild_dir / f"{origin}.APKBUILD",
            cands,
            size=meta["apkbuild_size"],
            sha256=meta["apkbuild_sha256"],
        )
        apkbuild_bytes = apkbuild_path.read_bytes()
        repo = meta["repo"]
        arc_apkbuild = f"aports/{repo}/{origin}/APKBUILD"
        entries[arc_apkbuild] = (apkbuild_bytes, 0o644)

        text = apkbuild_bytes.decode("utf-8", errors="replace")
        variables: dict[str, str] = {}
        for line in text.splitlines():
            s = line.strip()
            if re.match(r"^[A-Za-z_][A-Za-z0-9_]*\s*\(\)", s):
                break
            m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", s)
            if m and m.group(1) not in ("source", "sha512sums"):
                val = m.group(2).strip()
                if (val.startswith('"') and val.endswith('"')) or (val.startswith("'") and val.endswith("'")):
                    val = val[1:-1]
                elif "#" in val:
                    val = val.split("#", 1)[0].strip()
                variables[m.group(1)] = expand_apkbuild_vars(val, variables)

        source_urls: dict[str, str] = {}
        src_m = re.search(r'(?:^|\n)source=(?:"([^"]*)"|\'([^\']*)\'|([^\s#]+))', text)
        if src_m:
            for tok in next(g for g in src_m.groups() if g is not None).split():
                exp = expand_apkbuild_vars(tok.strip(), variables)
                local_name, url = exp.split("::", 1) if "::" in exp else (Path(urllib.parse.urlparse(exp).path).name, exp)
                if url.startswith(("https://", "http://")):
                    source_urls[local_name] = url

        copyleft = any(tag in meta["license"].upper() for tag in ("GPL", "LGPL", "AGPL"))
        fetched = []
        sums_m = re.search(r'(?:^|\n)sha512sums=(?:"([^"]*)"|\'([^\']*)\')', text)
        if copyleft and sums_m:
            for row in next(g for g in sums_m.groups() if g is not None).splitlines():
                parts = row.strip().split()
                if len(parts) != 2 or not SHA512_RE.fullmatch(parts[0]):
                    continue
                sha512, fname = parts[0], Path(parts[1]).name
                ensure(fname == parts[1] and fname not in ("", ".", ".."), f"unsafe distfile {parts[1]!r}")
                dest = distfiles_dir / f"{origin}__{fname}"
                dist_cands = [dest] + (
                    [cache_dir / "distfiles" / fname, cache_dir.parent / "distfiles" / fname]
                    if cache_dir
                    else []
                )
                matched = next(
                    (c for c in dist_cands if c.is_file() and not c.is_symlink() and file_digest(c, "sha512") == sha512),
                    None,
                )
                if matched is None:
                    raw_url = source_urls.get(fname)
                    is_tar = fname.endswith((".tar.gz", ".tar.xz", ".tar.bz2", ".tar.lz", ".tgz", ".txz", ".zip"))
                    try_urls: list[str] = []
                    if not is_tar and raw_url is None:
                        try_urls.append(f"{aports_base}/{repo}/{origin}/{urllib.parse.quote(fname)}")
                    try_urls.extend(f"{m_url}/{urllib.parse.quote(fname)}" for m_url in mirrors)
                    if raw_url and raw_url.startswith("https://"):
                        if raw_url.startswith("https://ftp.gnu.org/gnu/"):
                            try_urls.append(raw_url.replace("https://ftp.gnu.org/gnu/", "https://ftpmirror.gnu.org/gnu/", 1))
                        try_urls.append(raw_url)
                    if is_tar and raw_url is None:
                        try_urls.append(f"{aports_base}/{repo}/{origin}/{urllib.parse.quote(fname)}")
                    last_err = None
                    for u in try_urls:
                        try:
                            matched = https_fetch(u, dest, sha512=sha512)
                            break
                        except Exception as exc:  # noqa: BLE001
                            last_err = exc
                    ensure(matched is not None, f"failed fetching verified source {fname} for {origin}: {last_err}")
                arc_dist = f"distfiles/{origin}/{fname}"
                entries[arc_dist] = (matched, 0o644)
                fetched.append(
                    {
                        "path": arc_dist,
                        "sha512": sha512,
                        "sha256": file_digest(matched, "sha256"),
                        "size": matched.stat().st_size,
                    }
                )

        manifest_rows.append(
            {
                "origin": origin,
                "version": meta["version"],
                "license": meta["license"],
                "apkbuild": arc_apkbuild,
                "apkbuild_sha256": meta["apkbuild_sha256"],
                "copyleft_sources_bundled": copyleft,
                "files": fetched,
            }
        )

    entries["aports/ORIGINS-MANIFEST.json"] = (
        (json.dumps(manifest_rows, indent=2, sort_keys=True) + "\n").encode("utf-8"),
        0o644,
    )
    return entries


def qemu_helper_prefix(arch: str, lock: dict, work_dir: Path, cache_dir: Path | None) -> list[str] | None:
    if host_linux_arch() == arch:
        return None
    if arch == "arm64" and host_linux_arch() == "amd64":
        for name in ("qemu-aarch64-static", "qemu-aarch64"):
            found = shutil.which(name)
            if found:
                return [found]
        amd64_meta = lock["alpine"]["architectures"]["amd64"]
        helper_dir = work_dir / "host-qemu-helper"
        for pkg_name in amd64_meta["helper_packages"]:
            pkg = amd64_meta["packages"][pkg_name]
            cands = [cache_dir / "apks/x86_64" / pkg["filename"]] if cache_dir else []
            apk = obtain_locked(pkg["url"], work_dir / "apks/x86_64" / pkg["filename"], cands, size=pkg["size"], sha256=pkg["sha256"])
            extract_apk(apk, helper_dir)
        return [
            str(helper_dir / "lib/ld-musl-x86_64.so.1"),
            "--library-path",
            f"{helper_dir / 'lib'}:{helper_dir / 'usr/lib'}",
            str(helper_dir / "usr/bin/qemu-aarch64"),
        ]
    die(f"unsupported cross-arch host={host_linux_arch()} target={arch}")


def run_workbench_packer(
    inventory_file: Path, bundle_dir: Path, out_archive: Path, qemu_prefix: list[str] | None
) -> None:
    real_run = subprocess.run

    def maybe_emulated_run(cmd, *args, **kwargs):
        try:
            proc = real_run(cmd, *args, **kwargs)
        except OSError as exc:
            if exc.errno == errno.ENOEXEC and qemu_prefix is not None and isinstance(cmd, list):
                proc = real_run([*qemu_prefix, *cmd], *args, **kwargs)
            else:
                raise
        if isinstance(cmd, list) and cmd and Path(cmd[0]).name == "curl":
            ensure(
                b"HTTP3" in (proc.stdout + proc.stderr),
                "bundled network-workbench curl lacks required HTTP3 feature",
            )
        return proc

    old_argv = sys.argv[:]
    wb_packer.subprocess.run = maybe_emulated_run
    try:
        sys.argv = [
            str(TOOLS_DIR / "network_workbench_artifact.py"),
            "--inventory",
            str(inventory_file),
            "--bundle-root",
            str(bundle_dir),
            "--output",
            str(out_archive),
        ]
        wb_packer.main()
    finally:
        wb_packer.subprocess.run = real_run
        sys.argv = old_argv


def prepare_network_workbench(
    arch: str,
    lock: dict,
    musl_sysroot: Path,
    trampoline_src: Path,
    qemu_prefix: list[str] | None,
    artifact_root: Path,
    work_dir: Path,
    cache_dir: Path | None,
) -> dict[str, str]:
    arch_lock = lock["alpine"]["architectures"][arch]
    apk_arch = arch_lock["apk_arch"]
    ld_musl = arch_lock["ld_musl_name"]
    wb_lock = lock["external_sources"]["workbench"]
    py_ver = wb_lock["python_version"]

    wb_rootfs = work_dir / f"wb-rootfs-{arch}"
    shutil.rmtree(wb_rootfs, ignore_errors=True)
    origins: set[str] = set()
    pkg_notices = []
    for pkg_name in arch_lock["workbench_packages"]:
        pkg = arch_lock["packages"][pkg_name]
        origins.add(pkg["origin"])
        pkg_notices.append({k: pkg[k] for k in ("name", "version", "license", "origin", "upstream_url")})
        cands = [cache_dir / "apks" / apk_arch / pkg["filename"]] if cache_dir else []
        apk = obtain_locked(pkg["url"], work_dir / "apks" / apk_arch / pkg["filename"], cands, size=pkg["size"], sha256=pkg["sha256"])
        extract_apk(apk, wb_rootfs)

    curl_lock = lock["external_sources"]["static_curl"]
    cbin = curl_lock["binaries"][arch]
    curl_tar = obtain_locked(
        cbin["url"],
        work_dir / "upstream" / cbin["filename"],
        upstream_cache_candidates(cache_dir, cbin["filename"]),
        size=cbin["size"],
        sha256=cbin["sha256"],
    )
    curl_src = obtain_locked(
        curl_lock["source_url"],
        work_dir / "upstream" / curl_lock["source_filename"],
        upstream_cache_candidates(cache_dir, curl_lock["source_filename"]),
        size=curl_lock["source_size"],
        sha256=curl_lock["source_sha256"],
    )
    with tarfile.open(curl_tar, mode="r:xz") as tf:
        curl_member = next((m for m in tf.getmembers() if Path(m.name).name == "curl" and m.isfile()), None)
        ensure(curl_member is not None, "curl binary missing in static-curl archive")
        static_curl = tf.extractfile(curl_member).read()  # type: ignore[union-attr]
    verify_static_elf64(static_curl, arch, "static-curl")

    nt_lock = lock["external_sources"]["nexttrace"]
    nt_tar = obtain_locked(
        nt_lock["source_url"],
        work_dir / "upstream" / nt_lock["source_filename"],
        upstream_cache_candidates(cache_dir, nt_lock["source_filename"]),
        size=nt_lock["source_size"],
        sha256=nt_lock["source_sha256"],
    )
    nexttrace_bin, nexttrace_vendored = build_nexttrace(arch, nt_tar, nt_lock, work_dir, cache_dir)

    bundle_dir = work_dir / f"wb-bundle-{arch}"
    shutil.rmtree(bundle_dir, ignore_errors=True)
    stage_alpine_runtime_closure(wb_rootfs, bundle_dir, ld_musl, py_ver)

    drivedb = wb_rootfs / "usr/share/smartmontools/drivedb.h"
    if drivedb.is_file() and not drivedb.is_symlink():
        dst_db = bundle_dir / "share/smartmontools/drivedb.h"
        dst_db.parent.mkdir(parents=True, exist_ok=True)
        dst_db.write_bytes(drivedb.read_bytes())
        os.chmod(dst_db, 0o644)

    (bundle_dir / "libexec").mkdir(parents=True, exist_ok=True)
    (bundle_dir / "bin").mkdir(parents=True, exist_ok=True)
    py_elf = resolve_in_rootfs(wb_rootfs, f"usr/bin/python{py_ver}")
    (bundle_dir / "libexec/python3").write_bytes(py_elf.read_bytes())
    os.chmod(bundle_dir / "libexec/python3", 0o755)

    ld_rel = f"lib/{ld_musl}"
    py_trampoline = compile_trampoline(
        arch, musl_sysroot, trampoline_src, bundle_dir / "bin/python3",
        ld_rel=ld_rel, target_rel="libexec/python3", set_pythonhome=True,
    )
    for key, (ca, cb, libexec_rel) in WORKBENCH_DYNAMIC_BINS.items():
        src = next(
            (resolve_in_rootfs(wb_rootfs, c) for c in (ca, cb) if (wb_rootfs / c).exists() or (wb_rootfs / c).is_symlink()),
            None,
        )
        ensure(src is not None and src.is_file(), f"missing {key} in Alpine packages")
        (bundle_dir / libexec_rel).write_bytes(src.read_bytes())
        os.chmod(bundle_dir / libexec_rel, 0o755)
        compile_trampoline(
            arch, musl_sysroot, trampoline_src, bundle_dir / "bin" / key,
            ld_rel=ld_rel, target_rel=libexec_rel, set_mtr_packet=(key == "mtr"),
        )

    (bundle_dir / "bin/curl").write_bytes(static_curl)
    os.chmod(bundle_dir / "bin/curl", 0o755)
    (bundle_dir / "bin/nexttrace").write_bytes(nexttrace_bin)
    os.chmod(bundle_dir / "bin/nexttrace", 0o755)

    notices_bytes = (
        "APSP Network Workbench Offline Bundle Third-Party Notices\n"
        + json.dumps(
            {
                "schema": 1,
                "component": "network-workbench",
                "version": wb_lock["version"],
                "arch": arch,
                "corresponding_source_asset": f"network-workbench-{wb_lock['version']}-linux-{arch}-sources.tar.gz",
                "alpine_release": lock["alpine"]["release"],
                "aports_commit": lock["alpine"]["aports_commit"],
                "packages": pkg_notices,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")
    (bundle_dir / "THIRD_PARTY_NOTICES.txt").write_bytes(notices_bytes)
    os.chmod(bundle_dir / "THIRD_PARTY_NOTICES.txt", 0o644)

    reserved = {"bin/python3", "certs/ca-certificates.crt", "THIRD_PARTY_NOTICES.txt", *(f"bin/{n}" for n in wb_lock["tools"])}
    bundled_files = {}
    for cur_root, dirs, files in os.walk(bundle_dir, followlinks=False):
        dirs.sort()
        for fname in sorted(files):
            fpath = Path(cur_root) / fname
            rel = fpath.relative_to(bundle_dir).as_posix()
            if rel in reserved:
                continue
            check_bundled_path(rel)
            bundled_files[rel] = {
                "sha256": file_digest(fpath),
                "executable": bool(fpath.stat().st_mode & 0o111),
            }

    inventory = {
        "schema": 1,
        "engine_version": wb_lock["version"],
        "bundled_ca_bundle": "certs/ca-certificates.crt",
        "bundled_lib_dirs": ["lib"],
        "bundled_files": bundled_files,
        "interpreter": {
            "bundled_path": "bin/python3",
            "bundled_python_home": "python",
            "sha256": sha256_bytes(py_trampoline),
        },
        "tools": {
            name: {
                "bundled_path": f"bin/{name}",
                "version": meta["version"],
                "licensed": True,
                "license": meta["license"],
                "source_url": meta["source_url"],
                "sha256": file_digest(bundle_dir / "bin" / name),
            }
            for name, meta in sorted(wb_lock["tools"].items())
        },
    }
    inv_file = work_dir / f"wb-inventory-{arch}.json"
    inv_file.write_text(json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    staged_wb = work_dir / f"wb-{arch}.tar.gz"
    run_workbench_packer(inv_file, bundle_dir, staged_wb, qemu_prefix)

    wb_out_dir = artifact_root / "network-workbench" / wb_lock["version"]
    wb_payload = staged_wb.read_bytes()

    apk_sources = collect_apk_corresponding_sources(origins, lock["alpine"], work_dir, cache_dir)
    wb_sources: dict[str, tuple[bytes | Path, int]] = {
        **apk_sources,
        "apsp/apsp-network-workbench.py": (TOOLS_DIR / "network-workbench.py", 0o755),
        "apsp/network_workbench_artifact.py": (TOOLS_DIR / "network_workbench_artifact.py", 0o755),
        "apsp/trampoline.c": (trampoline_src, 0o644),
        "apsp/LICENSE": (REPO_ROOT / "LICENSE", 0o644),
        "apsp/THIRD_PARTY_NOTICES.txt": (notices_bytes, 0o644),
        f"upstream/{curl_lock['source_filename']}": (curl_src, 0o644),
        f"upstream/NTrace-core-{nt_lock['version']}-vendored.tar.gz": (nexttrace_vendored, 0o644),
    }
    wb_sources_path = wb_out_dir / f"{arch}.sources.tar.gz"
    wb_sources_sha = write_deterministic_tar_gz(wb_sources_path, wb_sources)
    wb_published = publish_idempotent(wb_out_dir, arch, wb_payload)
    return {
        "archive": str(wb_published),
        "archive_sha256": sha256_bytes(wb_payload),
        "sources": str(wb_sources_path),
        "sources_sha256": wb_sources_sha,
    }


def replace_once(blob: bytes, old: bytes, new: bytes, label: str) -> bytes:
    ensure(blob.count(old) == 1, f"expected unique anchor for {label}")
    return blob.replace(old, new, 1)


def build_canonical_nodequality_r22(
    arch: str, lock: dict, work_dir: Path, cache_dir: Path | None
) -> tuple[bytes, dict[str, bytes]]:
    nq_lock = lock["external_sources"]["nodequality"]
    plugin_dir = REPO_ROOT / "crates/observability/upstream/nodequality"
    raw_out = work_dir / f"nq-canonical-{arch}"
    shutil.rmtree(raw_out, ignore_errors=True)
    raw_out.mkdir(parents=True, exist_ok=True)

    run_env = dict(os.environ)
    if cache_dir is not None:
        cache_files_dir = next(
            (d for d in (cache_dir / "files", cache_dir.parent / "upstream-sources/files") if d.is_dir()),
            None,
        )
        if cache_files_dir is not None:
            dl_rows = subprocess.run(
                [sys.executable, str(plugin_dir / "source-helper.py"), "downloads", str(plugin_dir / "source-lock.json")],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.splitlines()
            case_arms = []
            for row in dl_rows:
                if "\t" not in row:
                    continue
                fname, surl = row.split("\t", 1)
                cached_f = cache_files_dir / fname
                if cached_f.is_file() and not cached_f.is_symlink():
                    case_arms.append(f"  {shlex.quote(surl)}) exec cat {shlex.quote(str(cached_f))} ;;")
            shim_dir = work_dir / "nq-curl-shim"
            shim_dir.mkdir(parents=True, exist_ok=True)
            real_curl = shutil.which("curl") or "/usr/bin/curl"
            shim = shim_dir / "curl"
            shim.write_text(
                "#!/bin/sh\n"
                "for last; do :; done\n"
                + ("case \"$last\" in\n" + "\n".join(case_arms) + "\nesac\n" if case_arms else "")
                + "base=${last##*/}\n"
                f"if [ -f {shlex.quote(str(cache_files_dir))}/\"$base\" ]; then exec cat {shlex.quote(str(cache_files_dir))}/\"$base\"; fi\n"
                f"exec {shlex.quote(real_curl)} \"$@\"\n",
                encoding="utf-8",
            )
            os.chmod(shim, 0o755)
            run_env["PATH"] = f"{shim_dir}:{run_env.get('PATH', '/usr/bin:/bin')}"

    subprocess.run(
        ["bash", str(TOOLS_DIR / "build-nodequality.sh"), arch, str(raw_out)],
        env=run_env,
        check=True,
    )
    canonical_tar = raw_out / "nodequality" / nq_lock["version"] / arch
    with tarfile.open(canonical_tar, mode="r:gz") as tf:
        runner = tf.extractfile("nodequality").read()  # type: ignore[union-attr]

    begin = b"<<'APSP_NODEQUALITY_PINNED_CHAIN'\n"
    end = b"\nAPSP_NODEQUALITY_PINNED_CHAIN\n"
    ensure(runner.count(begin) == 1 and runner.count(end) == 1, "missing pinned-chain sentinel")
    chain_bytes = runner[runner.index(begin) + len(begin) : runner.index(end) + 1]
    chain_file = work_dir / "nq-pinned-chain.json"
    chain_file.write_bytes(chain_bytes)
    mat_dir = work_dir / "nq-materialized-sources"
    shutil.rmtree(mat_dir, ignore_errors=True)
    subprocess.run(
        [sys.executable, str(plugin_dir / "source-helper.py"), "materialize", str(chain_file), str(mat_dir)],
        check=True,
    )
    upstream_files = {p.name: p.read_bytes() for p in sorted(mat_dir.iterdir()) if p.is_file()}

    nq_query = (plugin_dir / "node-query.py").read_bytes()
    nq_query = replace_once(
        nq_query,
        b"CURL = '/usr/bin/curl'\n",
        b"BUNDLED_CURL = os.environ.get('APSP_BUNDLED_CURL', '').strip()\n"
        b"BUNDLED_CA = os.environ.get('APSP_BUNDLED_CA', '').strip()\n"
        b"CURL = BUNDLED_CURL if (BUNDLED_CURL and os.path.isfile(BUNDLED_CURL)) else '/usr/bin/curl'\n",
        "node-query curl",
    )
    nq_query = replace_once(
        nq_query,
        b"    config += ['header = ' + quote(header) for header in headers]\n",
        b"    if BUNDLED_CA and CURL == BUNDLED_CURL and os.path.isfile(BUNDLED_CA):\n"
        b"        config.append('cacert = ' + quote(BUNDLED_CA))\n"
        b"    config += ['header = ' + quote(header) for header in headers]\n",
        "node-query ca",
    )

    runner = replace_once(runner, b"--mode daily|full", b"--mode daily|ip|full", "nq mode usage")
    runner = replace_once(
        runner,
        b"       nodequality --version\n",
        b"       IP mode requires --ips-file <WORKSPACE/node-ips.json> --job-id <UUID>.\n"
        b"       Official private credentials stay in /etc/apsp/nodequality-providers.json.\n"
        b"       nodequality --version\n",
        "nq ip usage",
    )
    runner = replace_once(runner, b"targets_file=\n", b"targets_file=\nips_file=\njob_id=\n", "nq ip vars")
    runner = replace_once(
        runner,
        b"    --targets-file) targets_file=$2 ;;\n",
        b"    --targets-file) targets_file=$2 ;;\n"
        b"    --ips-file) [[ -z $ips_file ]] || die 'duplicate IP input'; ips_file=$2 ;;\n"
        b"    --job-id) [[ -z $job_id ]] || die 'duplicate job identity'; job_id=$2 ;;\n",
        "nq ip flags",
    )
    runner = replace_once(
        runner,
        b"case \"$mode\" in daily|full) ;; *) die 'invalid diagnostic mode' ;; esac\n",
        b"case \"$mode\" in daily|ip|full) ;; *) die 'invalid diagnostic mode' ;; esac\n"
        b"if [[ $mode == ip ]]; then\n"
        b"  [[ $network_mode == low && $upload_report == false && -z $targets_file ]] || die 'IP queries require a bounded private job'\n"
        b"  [[ $ips_file == \"$workspace/node-ips.json\" && -n $job_id ]] || die 'IP input and job identity are required'\n"
        b"else\n"
        b"  [[ -z $ips_file && -z $job_id ]] || die 'IP options require IP mode'\n"
        b"fi\n",
        "nq ip check",
    )
    ip_block = (
        b"if [[ $mode == ip ]]; then\n"
        b"  cat > \"$runtime/node-query.py\" <<'APSP_OFFICIAL_NODE_QUERY'\n"
        + nq_query
        + b"APSP_OFFICIAL_NODE_QUERY\n"
        b"  python3 \"$runtime/node-query.py\" --workspace \"$workspace\" --ips-file \"$ips_file\" \\\n"
        b"    --ip-version \"$ip_version\" --job-id \"$job_id\"\n"
        b"  exit 0\n"
        b"fi\n"
    )
    runner = replace_once(
        runner,
        b'if [[ $mode == daily ]]; then\n  cat > "$runtime/official-ip.py"',
        ip_block + b'if [[ $mode == daily ]]; then\n  cat > "$runtime/official-ip.py"',
        "nq ip dispatch",
    )
    return runner, upstream_files


def prepare_nodequality_bundle(
    arch: str,
    lock: dict,
    musl_sysroot: Path,
    trampoline_src: Path,
    artifact_root: Path,
    work_dir: Path,
    cache_dir: Path | None,
) -> dict[str, str]:
    arch_lock = lock["alpine"]["architectures"][arch]
    apk_arch = arch_lock["apk_arch"]
    ld_musl = arch_lock["ld_musl_name"]
    nq_lock = lock["external_sources"]["nodequality"]
    py_ver = lock["external_sources"]["workbench"]["python_version"]

    nq_rootfs = work_dir / f"nq-rootfs-{arch}"
    shutil.rmtree(nq_rootfs, ignore_errors=True)
    origins: set[str] = set()
    pkg_notices = []
    for pkg_name in arch_lock["nodequality_packages"]:
        pkg = arch_lock["packages"][pkg_name]
        origins.add(pkg["origin"])
        pkg_notices.append({k: pkg[k] for k in ("name", "version", "license", "origin", "upstream_url")})
        cands = [cache_dir / "apks" / apk_arch / pkg["filename"]] if cache_dir else []
        apk = obtain_locked(pkg["url"], work_dir / "apks" / apk_arch / pkg["filename"], cands, size=pkg["size"], sha256=pkg["sha256"])
        extract_apk(apk, nq_rootfs)

    runner_r22, upstream_files = build_canonical_nodequality_r22(arch, lock, work_dir, cache_dir)

    bundle_dir = work_dir / f"nq-bundle-{arch}"
    shutil.rmtree(bundle_dir, ignore_errors=True)
    stage_alpine_runtime_closure(nq_rootfs, bundle_dir, ld_musl, py_ver)

    libexec_dir = bundle_dir / "libexec"
    corebin_dir = libexec_dir / "corebin"
    bin_dir = bundle_dir / "bin"
    corebin_dir.mkdir(parents=True, exist_ok=True)
    bin_dir.mkdir(parents=True, exist_ok=True)
    (libexec_dir / "nodequality-r22.sh").write_bytes(runner_r22)
    os.chmod(libexec_dir / "nodequality-r22.sh", 0o755)

    ld_rel = f"lib/{ld_musl}"
    for bname, cands_rel, py_home in (
        ("bash", ("bin/bash", "usr/bin/bash"), False),
        ("python3", (f"usr/bin/python{py_ver}", "usr/bin/python3"), True),
        ("curl", ("usr/bin/curl", "bin/curl"), False),
    ):
        src = next(
            (resolve_in_rootfs(nq_rootfs, c) for c in cands_rel if (nq_rootfs / c).exists() or (nq_rootfs / c).is_symlink()),
            None,
        )
        ensure(src is not None and src.is_file(), f"missing {bname} in nodequality rootfs")
        (libexec_dir / bname).write_bytes(src.read_bytes())
        os.chmod(libexec_dir / bname, 0o755)
        compile_trampoline(
            arch, musl_sysroot, trampoline_src, bin_dir / bname,
            ld_rel=ld_rel, target_rel=f"libexec/{bname}", set_pythonhome=py_home,
        )

    bb_src = resolve_in_rootfs(nq_rootfs, "bin/busybox")
    (libexec_dir / "busybox").write_bytes(bb_src.read_bytes())
    os.chmod(libexec_dir / "busybox", 0o755)
    for applet in NODEQUALITY_CORE_APPLETS:
        compile_trampoline(
            arch, musl_sysroot, trampoline_src, corebin_dir / applet,
            ld_rel=ld_rel, target_rel="libexec/busybox", self_depth=3,
        )

    wrapper = (
        "#!/bin/sh\n"
        "set -eu\n"
        "case \"$0\" in */*) root=${0%/*} ;; *) root=. ;; esac\n"
        "root=$(CDPATH= cd -- \"$root\" && pwd -P)\n"
        "PATH=\"$root/bin:${PATH:-/usr/bin:/bin}:$root/libexec/corebin\"\n"
        "SSL_CERT_FILE=\"$root/certs/ca-certificates.crt\"\n"
        "CURL_CA_BUNDLE=\"$root/certs/ca-certificates.crt\"\n"
        "TERMINFO=\"$root/share/terminfo\"\n"
        "APSP_BUNDLED_CURL=\"$root/bin/curl\"\n"
        "APSP_BUNDLED_CA=\"$root/certs/ca-certificates.crt\"\n"
        "export PATH SSL_CERT_FILE CURL_CA_BUNDLE TERMINFO APSP_BUNDLED_CURL APSP_BUNDLED_CA\n"
        "exec \"$root/bin/bash\" \"$root/libexec/nodequality-r22.sh\" \"$@\"\n"
    ).encode("utf-8")

    rt_manifest = (
        json.dumps(
            {
                "schema": 1,
                "component": "nodequality",
                "version": nq_lock["version"],
                "arch": arch,
                "full_start_allowed": False,
                "modes_enabled": ["daily", "ip"],
                "bundled_helpers": ["bin/bash", "bin/python3", "bin/curl"],
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")
    notices = (
        "APSP Self-Contained NodeQuality -r22 Third-Party Notices\n"
        + json.dumps(
            {
                "schema": 1,
                "component": "nodequality",
                "version": nq_lock["version"],
                "arch": arch,
                "corresponding_source_asset": f"nodequality-{nq_lock['version']}-linux-{arch}-sources.tar.gz",
                "full_start_allowed": False,
                "alpine_release": lock["alpine"]["release"],
                "aports_commit": lock["alpine"]["aports_commit"],
                "packages": pkg_notices,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")

    entries: dict[str, tuple[bytes, int]] = {
        "nodequality": (wrapper, 0o755),
        "runtime-manifest.json": (rt_manifest, 0o644),
        "LICENSE": (upstream_files["LICENSE.nodequality"], 0o644),
        "THIRD_PARTY_NOTICES.txt": (notices, 0o644),
    }
    for cur_root, dirs, files in os.walk(bundle_dir, followlinks=False):
        dirs.sort()
        for fname in sorted(files):
            fpath = Path(cur_root) / fname
            rel = fpath.relative_to(bundle_dir).as_posix()
            check_bundled_path(rel)
            entries[rel] = (fpath.read_bytes(), 0o755 if (fpath.stat().st_mode & 0o111) else 0o644)

    nq_out_dir = artifact_root / "nodequality" / nq_lock["version"]
    nq_payload = pack_deterministic_tar_gz(entries)

    apk_sources = collect_apk_corresponding_sources(origins, lock["alpine"], work_dir, cache_dir)
    plugin_dir = REPO_ROOT / "crates/observability/upstream/nodequality"
    nq_sources: dict[str, tuple[bytes | Path, int]] = {
        **apk_sources,
        "apsp/trampoline.c": (trampoline_src, 0o644),
        "apsp/LICENSE": (REPO_ROOT / "LICENSE", 0o644),
        "apsp/THIRD_PARTY_NOTICES.txt": (notices, 0o644),
        "apsp/runtime-manifest.json": (rt_manifest, 0o644),
        **{
            f"apsp/nodequality/{p.name}": (p, 0o755 if (p.stat().st_mode & 0o111) else 0o644)
            for p in sorted(plugin_dir.iterdir(), key=lambda x: x.name)
            if p.is_file() and not p.is_symlink()
        },
        **{f"upstream/nodequality/{k}": (v, 0o644) for k, v in sorted(upstream_files.items())},
    }
    nq_sources_path = nq_out_dir / f"{arch}.sources.tar.gz"
    nq_sources_sha = write_deterministic_tar_gz(nq_sources_path, nq_sources)
    nq_published = publish_idempotent(nq_out_dir, arch, nq_payload)
    return {
        "archive": str(nq_published),
        "archive_sha256": sha256_bytes(nq_payload),
        "sources": str(nq_sources_path),
        "sources_sha256": nq_sources_sha,
    }


def cmd_prepare_bundled_tools(args: argparse.Namespace) -> None:
    lock = json.loads(args.lock_file.read_text(encoding="utf-8"))
    ensure(lock.get("schema") == 1, "unsupported agent-linux-tools-lock.json schema")
    arch = args.arch
    artifact_root = args.artifact_root.resolve()
    work_dir = args.work_dir.resolve()
    cache_dir = args.cache_dir.resolve() if args.cache_dir is not None else None
    work_dir.mkdir(parents=True, exist_ok=True)
    artifact_root.mkdir(parents=True, exist_ok=True)

    trampoline_src = work_dir / "trampoline.c"
    trampoline_src.write_text(TRAMPOLINE_C, encoding="utf-8")

    arch_lock = lock["alpine"]["architectures"][arch]
    apk_arch = arch_lock["apk_arch"]
    musl_sysroot = work_dir / f"musl-sysroot-{arch}"
    shutil.rmtree(musl_sysroot, ignore_errors=True)
    for pkg_name in ("musl", "musl-dev"):
        pkg = arch_lock["packages"][pkg_name]
        cands = [cache_dir / "apks" / apk_arch / pkg["filename"]] if cache_dir else []
        apk = obtain_locked(pkg["url"], work_dir / "apks" / apk_arch / pkg["filename"], cands, size=pkg["size"], sha256=pkg["sha256"])
        extract_apk(apk, musl_sysroot)

    qemu_prefix = qemu_helper_prefix(arch, lock, work_dir, cache_dir)
    wb = prepare_network_workbench(arch, lock, musl_sysroot, trampoline_src, qemu_prefix, artifact_root, work_dir, cache_dir)
    nq = prepare_nodequality_bundle(arch, lock, musl_sysroot, trampoline_src, artifact_root, work_dir, cache_dir)
    print(json.dumps({"arch": arch, "network_workbench": wb, "nodequality": nq}, indent=2, sort_keys=True))


def cmd_package_singbox_sources(args: argparse.Namespace) -> None:
    lock = json.loads(args.lock_file.read_text(encoding="utf-8"))
    sb_lock = lock["external_sources"]["singbox"]
    version, commit = sb_lock["version"], sb_lock["upstream_commit"]
    artifact_root, work_dir = args.artifact_root.resolve(), args.work_dir.resolve()
    work_dir.mkdir(parents=True, exist_ok=True)

    if args.source_dir is not None:
        checkout = args.source_dir.resolve()
        ensure(checkout.is_dir(), f"missing sing-box checkout: {checkout}")
    else:
        checkout = work_dir / "sing-box-upstream"
        shutil.rmtree(checkout, ignore_errors=True)
        subprocess.run(
            ["git", "clone", "--quiet", "--depth", "1", "--branch", sb_lock["upstream_tag"], sb_lock["upstream_repo"], str(checkout)],
            check=True,
        )

    head = subprocess.run(["git", "-C", str(checkout), "rev-parse", "--verify", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    dirty = subprocess.run(["git", "-C", str(checkout), "status", "--porcelain"], capture_output=True, text=True, check=True).stdout.strip()
    ensure(head == commit and not dirty, f"sing-box checkout must be clean at {commit}")

    git_tar = subprocess.run(
        ["git", "-C", str(checkout), "archive", "--format=tar", f"--prefix=sing-box-{version}/upstream/", commit],
        capture_output=True,
        check=True,
    ).stdout
    entries: dict[str, tuple[bytes, int]] = {}
    with tarfile.open(fileobj=io.BytesIO(git_tar), mode="r:") as tf:
        for m in tf.getmembers():
            if m.isfile():
                entries[m.name] = (tf.extractfile(m).read(), 0o755 if (m.mode & 0o111) else 0o644)  # type: ignore[union-attr]

    for rel in (Path("LICENSE"), Path("tools/build-singbox.sh"), Path("tools/singbox-builder.Dockerfile")):
        src = REPO_ROOT / rel
        entries[f"sing-box-{version}/apsp/{rel.as_posix()}"] = (src.read_bytes(), 0o755 if (src.stat().st_mode & 0o111) else 0o644)
    for cur_root, dirs, files in os.walk(REPO_ROOT / "tools/singbox-runtime", followlinks=False):
        dirs.sort()
        for fname in sorted(files):
            fpath = Path(cur_root) / fname
            if fpath.is_file() and not fpath.is_symlink():
                rel_p = fpath.relative_to(REPO_ROOT).as_posix()
                entries[f"sing-box-{version}/apsp/{rel_p}"] = (fpath.read_bytes(), 0o755 if (fpath.stat().st_mode & 0o111) else 0o644)

    entries[f"sing-box-{version}/SOURCE-MANIFEST.json"] = (
        (json.dumps({"schema": 1, "component": "sing-box", **sb_lock, "license": "GPL-3.0-or-later"}, indent=2, sort_keys=True) + "\n").encode("utf-8"),
        0o644,
    )
    out_file = artifact_root / "sing-box" / version / f"sing-box-{version}-sources.tar.gz"
    sha = atomic_write(out_file, pack_deterministic_tar_gz(entries))
    print(json.dumps({"artifact": str(out_file), "version": version, "sha256": sha, "upstream_commit": commit}, indent=2, sort_keys=True))


def is_native_debian12_root(arch: str) -> bool:
    if os.geteuid() != 0 or host_linux_arch() != arch:
        return False
    os_release = Path("/etc/os-release")
    if not os_release.is_file():
        return False
    text = os_release.read_text(encoding="utf-8", errors="replace")
    return "ID=debian" in text and 'VERSION_ID="12"' in text


def run_ipquality_factory_native(
    arch: str,
    builder_sha: str,
    req_file: Path,
    keyring: Path,
    keyring_prov: Path,
    license_review: Path,
    source_bundle: Path,
    work_dir: Path,
    out_root: Path,
) -> None:
    spec = importlib.util.spec_from_file_location("ipq_rootfs", TOOLS_DIR / "ipquality-rootfs.py")
    ensure(spec is not None and spec.loader is not None, "unable to load tools/ipquality-rootfs.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    factory = mod.factory()

    deadline = factory.Deadline(120)
    tools = []
    for name, p in sorted(factory.TOOL_PATHS.items()):
        ident = factory.file_identity(p, factory.MAX_ARCHIVE, deadline)
        ver = factory.run_bounded([p, "--version"], factory.Deadline(15), 65536).decode("utf-8", errors="replace").strip()
        tools.append({"name": name, "path": p, "version": (ver.splitlines()[0] if ver else name)[:128], **ident})

    stage = work_dir / "factory-run"
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True, mode=0o700)
    os.chmod(stage, 0o700)
    cand = stage / "candidate-builder.json"
    cand.write_text(json.dumps({"image_sha256": builder_sha, "arch": arch, "tools": tools}, sort_keys=True) + "\n", encoding="utf-8")
    os.chmod(cand, 0o600)

    cli = [sys.executable, str(TOOLS_DIR / "ipquality-rootfs.py")]
    c1, c2, c3, c4, c5, c6 = (stage / n for n in ("01-col", "02-der", "03-bnd", "04-prep", "05-tree", "06-exp"))
    out_root.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(out_root, 0o700)
    subprocess.run([*cli, "collect", "--request", str(req_file), "--keyring", str(keyring), "--keyring-provenance", str(keyring_prov), "--candidate-builder", str(cand), "--output", str(c1), "--max-total-bytes", str(3 * 1024**3), "--timeout-seconds", "1800"], check=True)
    subprocess.run([*cli, "derive", "--materials", str(c1), "--output", str(c2), "--max-output-bytes", str(256 * 1024**2), "--timeout-seconds", "600"], check=True)
    subprocess.run([*cli, "bind", "--derived-inputs", str(c2), "--candidate-builder", str(cand), "--output", str(c3), "--timeout-seconds", "600"], check=True)
    subprocess.run([*cli, "prepare", "--lock", str(c3 / "inputs-lock.json"), "--cache", str(c1 / "input-cache"), "--derived-inputs", str(c2), "--derived-binding", str(c3), "--output", str(c4), "--approved-builder-image-sha256", builder_sha], check=True)
    subprocess.run([*cli, "build", "--prepared", str(c4), "--output", str(c5), "--approved-builder-image-sha256", builder_sha], check=True)
    subprocess.run([*cli, "export", "--tree", str(c5), "--prepared", str(c4), "--output", str(c6), "--outer-reserve-bytes", str(32 * 1024**2), "--approved-builder-image-sha256", builder_sha], check=True)
    subprocess.run([sys.executable, str(TOOLS_DIR / "build-ipquality.py"), "--arch", arch, "--source-bundle", str(source_bundle), "--prepared-directory", str(c4), "--rootfs-directory", str(c6), "--approved-builder-image-sha256", builder_sha, "--license-review", str(license_review), "--output", str(out_root)], check=True)


def cmd_prepare_ipquality(args: argparse.Namespace) -> None:
    lock = json.loads(args.lock_file.read_text(encoding="utf-8"))
    ipq_lock = lock["external_sources"]["ipquality"]
    arch = args.arch
    builder_sha = args.approved_builder_image_sha256.strip()
    ensure(
        bool(SHA256_RE.fullmatch(builder_sha)),
        "official IPQuality build requires 64-hex --approved-builder-image-sha256",
    )
    for flag, fpath in (
        ("--license-review-file", args.license_review_file),
        ("--keyring-provenance-file", args.keyring_provenance_file),
    ):
        ensure(
            fpath.is_file() and not fpath.is_symlink() and fpath.stat().st_size > 0,
            f"official IPQuality build requires {flag}: {fpath}",
        )

    artifact_root, work_dir = args.artifact_root.resolve(), args.work_dir.resolve()
    work_dir.mkdir(parents=True, exist_ok=True)
    artifact_root.mkdir(parents=True, exist_ok=True)
    req_file = (REPO_ROOT / ipq_lock["rootfs_requests"][arch]).resolve()

    if args.inside_builder:
        keyring = args.keyring_file.resolve() if args.keyring_file else Path("/usr/share/keyrings/debian-archive-keyring.gpg")
        ensure(args.source_bundle is not None and keyring.is_file(), "missing source-bundle or Debian keyring inside builder")
        run_ipquality_factory_native(
            arch, builder_sha, req_file, keyring,
            args.keyring_provenance_file.resolve(),
            args.license_review_file.resolve(),
            args.source_bundle.resolve(),
            work_dir, artifact_root,
        )
        return

    if args.source_bundle is not None:
        source_bundle = args.source_bundle.resolve()
        ensure(source_bundle.is_file() and not source_bundle.is_symlink(), f"invalid --source-bundle: {source_bundle}")
    else:
        helper = REPO_ROOT / "crates/observability/upstream/ipquality/source-helper.py"
        slock = REPO_ROOT / ipq_lock["source_lock_relpath"]
        dl_dir = work_dir / "ipquality-upstream"
        dl_dir.mkdir(parents=True, exist_ok=True)
        rows = subprocess.run([sys.executable, str(helper), "downloads", str(slock)], capture_output=True, text=True, check=True).stdout.splitlines()
        for row in rows:
            if not row.strip():
                continue
            fname, url = row.split("\t")
            tmp_f = dl_dir / f".raw.{fname}"
            https_fetch(url, tmp_f, max_bytes=2 * 1024 * 1024)
            subprocess.run([sys.executable, str(helper), "receive", str(dl_dir / fname)], input=tmp_f.read_bytes(), check=True, stdout=subprocess.DEVNULL)
            tmp_f.unlink(missing_ok=True)
        packed = subprocess.run([sys.executable, str(helper), "pack", str(slock), str(dl_dir)], capture_output=True, check=True).stdout
        source_bundle = work_dir / "ipquality-source-bundle.json"
        source_bundle.write_bytes(packed)
        os.chmod(source_bundle, 0o600)

    pub_stage = work_dir / "published-stage"
    shutil.rmtree(pub_stage, ignore_errors=True)
    if is_native_debian12_root(arch):
        keyring = args.keyring_file.resolve() if args.keyring_file else Path("/usr/share/keyrings/debian-archive-keyring.gpg")
        ensure(keyring.is_file(), f"missing Debian archive keyring: {keyring}")
        run_ipquality_factory_native(
            arch, builder_sha, req_file, keyring,
            args.keyring_provenance_file.resolve(),
            args.license_review_file.resolve(),
            source_bundle, work_dir, pub_stage,
        )
    else:
        ensure(shutil.which("docker") is not None, "prepare-ipquality requires root on approved Debian 12 builder or Docker")
        staged = work_dir / "staged-governance"
        shutil.rmtree(staged, ignore_errors=True)
        staged.mkdir(parents=True, mode=0o700)
        for name, src in (
            ("license-review.json", args.license_review_file),
            ("keyring-provenance.json", args.keyring_provenance_file),
            ("ipquality-source-bundle.json", source_bundle),
        ):
            shutil.copyfile(src, staged / name)
            os.chmod(staged / name, 0o600)
        kr_args = []
        if args.keyring_file is not None:
            shutil.copyfile(args.keyring_file, staged / "debian-archive-keyring.gpg")
            kr_args = ["--keyring-file", "/work/staged-governance/debian-archive-keyring.gpg"]
        image = (
            args.builder_image
            or os.environ.get(f"APSP_IPQUALITY_BUILDER_IMAGE_{arch.upper()}", "").strip()
            or os.environ.get("APSP_IPQUALITY_BUILDER_IMAGE", "").strip()
            or f"debian@sha256:{builder_sha}"
        )
        subprocess.run(
            [
                "docker", "run", "--rm", "--privileged", f"--platform=linux/{arch}",
                "-v", f"{REPO_ROOT}:/repo:ro", "-v", f"{work_dir}:/work", image,
                "python3", "/repo/tools/prepare-agent-linux-inputs.py", "prepare-ipquality",
                "--inside-builder", "--arch", arch,
                "--artifact-root", "/work/published-stage",
                "--work-dir", "/work/inner",
                "--approved-builder-image-sha256", builder_sha,
                "--license-review-file", "/work/staged-governance/license-review.json",
                "--keyring-provenance-file", "/work/staged-governance/keyring-provenance.json",
                "--source-bundle", "/work/staged-governance/ipquality-source-bundle.json",
                *kr_args,
            ],
            check=True,
        )

    produced = pub_stage / "ipquality" / ipq_lock["version"]
    dest_dir = artifact_root / "ipquality" / ipq_lock["version"]
    dest_dir.mkdir(parents=True, exist_ok=True)
    for item_name in (arch, f"{arch}.sources.tar.gz", "SHA256SUMS"):
        src_item = produced / item_name
        ensure(src_item.is_file(), f"IPQuality factory did not produce {src_item}")
        if item_name == "SHA256SUMS" and (dest_dir / "SHA256SUMS").is_file():
            merged = {}
            for p in (dest_dir / "SHA256SUMS", src_item):
                for line in p.read_text(encoding="utf-8").splitlines():
                    if line.strip():
                        d, k = line.split()
                        if (dest_dir / k).is_file() or (produced / k).is_file():
                            merged[k] = d
            atomic_write(dest_dir / "SHA256SUMS", "".join(f"{merged[k]}  {k}\n" for k in sorted(merged)).encode("utf-8"))
        else:
            shutil.copyfile(src_item, dest_dir / item_name)
            os.chmod(dest_dir / item_name, 0o644)

    print(
        json.dumps(
            {
                "arch": arch,
                "archive": str(dest_dir / arch),
                "archive_sha256": file_digest(dest_dir / arch),
                "sources": str(dest_dir / f"{arch}.sources.tar.gz"),
                "sources_sha256": file_digest(dest_dir / f"{arch}.sources.tar.gz"),
            },
            indent=2,
            sort_keys=True,
        )
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="subcommand", required=True)

    sb = sub.add_parser("package-singbox-sources")
    sb.add_argument("--artifact-root", type=Path, required=True)
    sb.add_argument("--work-dir", type=Path, required=True)
    sb.add_argument("--source-dir", type=Path, default=None)
    sb.add_argument("--lock-file", type=Path, default=DEFAULT_LOCK_FILE)

    bt = sub.add_parser("prepare-bundled-tools")
    bt.add_argument("--arch", choices=("amd64", "arm64"), required=True)
    bt.add_argument("--artifact-root", type=Path, required=True)
    bt.add_argument("--work-dir", type=Path, required=True)
    bt.add_argument("--cache-dir", type=Path, default=None)
    bt.add_argument("--lock-file", type=Path, default=DEFAULT_LOCK_FILE)

    ipq = sub.add_parser("prepare-ipquality")
    ipq.add_argument("--arch", choices=("amd64", "arm64"), required=True)
    ipq.add_argument("--artifact-root", type=Path, required=True)
    ipq.add_argument("--work-dir", type=Path, required=True)
    ipq.add_argument("--approved-builder-image-sha256", required=True)
    ipq.add_argument("--license-review-file", type=Path, required=True)
    ipq.add_argument("--keyring-provenance-file", type=Path, required=True)
    ipq.add_argument("--keyring-file", type=Path, default=None)
    ipq.add_argument("--source-bundle", type=Path, default=None)
    ipq.add_argument("--builder-image", default=None)
    ipq.add_argument("--inside-builder", action="store_true", help=argparse.SUPPRESS)
    ipq.add_argument("--lock-file", type=Path, default=DEFAULT_LOCK_FILE)
    return parser


def main() -> None:
    os.umask(0o022)
    args = build_parser().parse_args()
    if args.subcommand == "package-singbox-sources":
        cmd_package_singbox_sources(args)
    elif args.subcommand == "prepare-bundled-tools":
        cmd_prepare_bundled_tools(args)
    elif args.subcommand == "prepare-ipquality":
        cmd_prepare_ipquality(args)
    else:
        die(f"unsupported subcommand: {args.subcommand}")


if __name__ == "__main__":
    main()

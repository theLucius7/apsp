"""Strict derivation and inventory for the explicit official node-query runner."""
import gzip
import io
import tarfile

import nodequality_rootfs_artifact as canonical

ROOT = canonical.ROOT
PLUGIN = canonical.PLUGIN
VERSION = "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r21"
BINARY = "nodequality"
FILES = {BINARY}
MAX_RUNNER = canonical.MAX_RUNNER
MAX_ARCHIVE = MAX_RUNNER + 10240
ensure = canonical.ensure
digest = canonical.digest
replace_once = canonical.replace_once


def runner(base):
    ensure(isinstance(base, bytes) and 0 < len(base) <= MAX_RUNNER,
           "invalid canonical runner size")
    bundle = canonical.embedded(base, canonical.MARKERS["PINNED_CHAIN"])
    ensure(base == canonical.canonical_runner(bundle),
           "node query requires the unchanged canonical r19 runner")
    helper = canonical.runtime().ordinary(PLUGIN / "node-query.py", 128 * 1024)
    ensure(helper.endswith(b"\n") and b"\nAPSP_OFFICIAL_NODE_QUERY\n" not in helper,
           "invalid official node-query payload")
    result = replace_once(base, ("version=" + canonical.CANONICAL_VERSION + "\n").encode(),
                          ("version=" + VERSION + "\n").encode())
    result = replace_once(result, b"--mode daily|full", b"--mode daily|ip|full")
    result = replace_once(result, b"       nodequality --version\n",
                          b"       IP mode requires --ips-file <WORKSPACE/node-ips.json> --job-id <UUID>.\n"
                          b"       Official private credentials stay in /etc/apsp/nodequality-providers.json.\n"
                          b"       nodequality --version\n")
    result = replace_once(result, b"targets_file=\n", b"targets_file=\nips_file=\njob_id=\n")
    result = replace_once(result, b"    --targets-file) targets_file=$2 ;;\n",
                          b"    --targets-file) targets_file=$2 ;;\n"
                          b"    --ips-file) [[ -z $ips_file ]] || die 'duplicate IP input'; ips_file=$2 ;;\n"
                          b"    --job-id) [[ -z $job_id ]] || die 'duplicate job identity'; job_id=$2 ;;\n")
    result = replace_once(result,
        b'case "$mode" in daily|full) ;; *) die \'invalid diagnostic mode\' ;; esac\n',
        b'case "$mode" in daily|ip|full) ;; *) die \'invalid diagnostic mode\' ;; esac\n'
        b'if [[ $mode == ip ]]; then\n'
        b'  [[ $network_mode == low && $upload_report == false && -z $targets_file ]] || die \'IP queries require a bounded private job\'\n'
        b'  [[ $ips_file == "$workspace/node-ips.json" && -n $job_id ]] || die \'IP input and job identity are required\'\n'
        b'else\n'
        b'  [[ -z $ips_file && -z $job_id ]] || die \'IP options require IP mode\'\n'
        b'fi\n')
    injection = b'''if [[ $mode == ip ]]; then
  cat > "$runtime/node-query.py" <<'APSP_OFFICIAL_NODE_QUERY'
''' + helper + b'''APSP_OFFICIAL_NODE_QUERY
  python3 "$runtime/node-query.py" --workspace "$workspace" --ips-file "$ips_file" \\
    --ip-version "$ip_version" --job-id "$job_id"
  exit 0
fi
'''
    result = replace_once(result, b'if [[ $mode == daily ]]; then\n  cat > "$runtime/official-ip.py"',
                          injection + b'if [[ $mode == daily ]]; then\n  cat > "$runtime/official-ip.py"')
    ensure(len(result) <= MAX_RUNNER, "official query runner exceeds byte budget")
    return result


def validate_files(files, version, arch):
    ensure(version == VERSION and arch in ("amd64", "arm64") and set(files) == FILES,
           "wrong official node-query artifact identity or inventory")
    content = files[BINARY]
    ensure(isinstance(content, bytes) and 0 < len(content) <= MAX_RUNNER,
           "official node-query runner exceeds byte budget")
    bundle = canonical.embedded(content, canonical.MARKERS["PINNED_CHAIN"])
    ensure(content == runner(canonical.canonical_runner(bundle)),
           "official node-query runner differs from its controlled derivation")


def archive_files(data):
    ensure(isinstance(data, bytes) and 0 < len(data) <= MAX_ARCHIVE,
           "official node-query archive exceeds byte budget")
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        unpacked = compressed.read(MAX_ARCHIVE + 1)
    ensure(len(unpacked) <= MAX_ARCHIVE, "official query decompression exceeds byte budget")
    files = {}
    with tarfile.open(fileobj=io.BytesIO(unpacked), mode="r:") as archive:
        for member in archive:
            ensure(member.name in FILES and member.name not in files and member.isfile()
                   and not member.pax_headers and 0 < member.size <= MAX_RUNNER,
                   "unsafe official node-query archive member")
            files[member.name] = archive.extractfile(member).read(member.size + 1)
            ensure(len(files[member.name]) == member.size, "official query archive is truncated")
    ensure(set(files) == FILES, "official query artifact inventory is incomplete")
    return files


def pack(files):
    ensure(set(files) == FILES and 0 < len(files[BINARY]) <= MAX_RUNNER,
           "invalid official query package inventory")
    output = io.BytesIO()
    with gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=0, compresslevel=9) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            member = tarfile.TarInfo(BINARY)
            member.size, member.mode = len(files[BINARY]), 0o755
            archive.addfile(member, io.BytesIO(files[BINARY]))
    data = output.getvalue()
    ensure(len(data) <= MAX_ARCHIVE, "compressed official query archive exceeds byte budget")
    return data

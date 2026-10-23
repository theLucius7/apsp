#!/usr/bin/env python3
"""Stage a native TCP test recipe for a separately authorized isolated runner."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def git(source, *arguments):
    return subprocess.check_output(["git", "-C", str(source), *arguments], timeout=120)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        while value := source.read(1024 * 1024):
            result.update(value)
    return result.hexdigest()


RUNNER = r'''#!/bin/sh
# Run only inside a new private Debian 12 amd64 container after source freeze.
set -eu
umask 077
[ "${APSP_REMAINING_TEST_SIGNAL:-}" = 1 ] || exit 2
[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] || exit 2
[ -f /inputs/source.bundle ] && [ -f /inputs/plan.json ] || exit 2
[ -d /output ] && [ ! -e /output/source ] && [ ! -e /output/receipt.json ] || exit 2
for program in git cargo rustc cc musl-gcc python3 minisign; do
  command -v "$program" >/dev/null || exit 2
done
export PYTHONDONTWRITEBYTECODE=1 CARGO_NET_OFFLINE=true
export CARGO_TARGET_DIR=/output/target CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
commit=$(python3 -c 'import json; print(json.load(open("/inputs/plan.json"))["source_commit"])')
python3 - <<'PY'
import hashlib,json,re
from pathlib import Path
os_release=dict(line.split('=',1) for line in Path('/etc/os-release').read_text().splitlines() if '=' in line)
assert os_release.get('ID','').strip('"')=='debian'
assert os_release.get('VERSION_ID','').strip('"')=='12'
p=json.loads(Path('/inputs/plan.json').read_text())
assert re.fullmatch('[0-9a-f]{40}',p['source_commit'])
assert hashlib.sha256(Path('/inputs/source.bundle').read_bytes()).hexdigest()==p['bundle_sha256']
assert hashlib.sha256(Path('/inputs/run-native.sh').read_bytes()).hexdigest()==p['runner_sha256']
PY
exec > /output/native.log 2>&1
finish() {
  result=$?
  trap - EXIT
  python3 - "$result" "$commit" <<'PY'
import json,sys,time
from pathlib import Path
status,commit=sys.argv[1:]
Path('/output/receipt.json').write_text(json.dumps({'source_commit':commit,'exit_code':int(status),
 'finished_at':int(time.time()),'test_only':True,'scope':'native TCP complete package tests and fixed musl artifact, not plugin lifecycle/production'},indent=2)+'\n')
PY
  exit "$result"
}
trap finish EXIT
git clone /inputs/source.bundle /output/source
git -C /output/source checkout --detach "$commit"
cd /output/source
test "$(git rev-parse HEAD)" = "$commit"
test -z "$(git status --porcelain --untracked-files=normal)"
rustc -vV > /output/rustc-version.log
cargo -V > /output/cargo-version.log
rustc --print sysroot > /output/rust-sysroot.log
APSP_RELEASE_PUBLIC_KEYS=$(python3 scripts/ci-test-trust.py)
export APSP_RELEASE_PUBLIC_KEYS
export APSP_NATIVE_TCP_SOURCE_COMMIT="$commit"
cargo fmt --all --check
python3 tools/check-core-boundary.py
cargo test --locked --offline -p apsp-tcp-probe -- --test-threads=1 > /output/tcp-tests.log 2>&1
python3 - <<'PY'
import json,re
from pathlib import Path
rows=re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;', Path('/output/tcp-tests.log').read_text())
assert rows and sum(int(row[1]) for row in rows)>0, 'native Rust package tests were not observed'
assert all(row[0]=='ok' and all(int(value)==0 for value in row[2:]) for row in rows), 'incomplete native Rust test result'
Path('/output/tcp-tests.json').write_text(json.dumps({'tests':sum(int(row[1]) for row in rows), 'failures':0,'ignored':0,'filtered_out':0,'groups':len(rows)},indent=2)+'\n')
PY
cargo clippy --locked --offline -p apsp-tcp-probe --all-targets -- -D warnings
run_python_suite() {
  python3 - "$1" "$2" <<'PY'
import json,sys,unittest
from pathlib import Path
suite=unittest.defaultTestLoader.discover('tests',pattern=sys.argv[1])
with Path('/output/'+sys.argv[2]+'.log').open('x') as log:
 result=unittest.TextTestRunner(stream=log,verbosity=2).run(suite)
receipt={'tests':result.testsRun,'failures':len(result.failures),'errors':len(result.errors),'skips':len(result.skipped)}
Path('/output/'+sys.argv[2]+'.json').write_text(json.dumps(receipt,indent=2)+'\n')
raise SystemExit(0 if result.testsRun>0 and result.wasSuccessful() and not result.skipped else 1)
PY
}
run_python_suite test_tcp_probe_artifacts.py tcp-artifact-tests
python3 tools/build-tcp-probe.py amd64 /output/artifacts --source-commit "$commit"
export APSP_TCP_ARCH=amd64 APSP_TCP_SOURCE_COMMIT="$commit" APSP_TCP_ARTIFACT_ROOT=/output/artifacts
run_python_suite test_tcp_probe_native_bundle.py tcp-native-bundle-tests
test -z "$(git status --porcelain --untracked-files=normal)"
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(re.fullmatch(r"[0-9a-f]{40}", args.source_commit), "explicit full frozen source commit required")
    source = args.source.absolute()
    require(git(source, "rev-parse", "HEAD").decode().strip() == args.source_commit, "HEAD differs from freeze")
    require(not git(source, "status", "--porcelain", "--untracked-files=normal"), "requires clean committed source")
    output = args.output.absolute()
    require(not output.exists() and not output.is_symlink(), "new output required; preserve old attempts")
    require(output.parent.is_dir() and not any(path.is_symlink() for path in (output.parent, *output.parents)),
            "ordinary private output parent required")
    require(stat.S_IMODE(output.parent.stat().st_mode) & 0o077 == 0, "output parent must be mode 0700")
    os.umask(0o077)
    output.mkdir(mode=0o700)
    bundle = output / "source.bundle"
    git(source, "bundle", "create", str(bundle), "HEAD")
    require(bundle.stat().st_size <= 128 * 1024 * 1024, "source bundle exceeds private staging limit")
    git(source, "bundle", "verify", str(bundle))
    require(git(source, "rev-parse", "HEAD").decode().strip() == args.source_commit
            and not git(source, "status", "--porcelain", "--untracked-files=normal"), "source changed during freeze")
    runner = output / "run-native.sh"
    with runner.open("x") as destination:
        destination.write(RUNNER)
    runner.chmod(0o700)
    plan = {"schema": 1, "status": "prepared_not_executed", "source_commit": args.source_commit,
            "bundle_sha256": digest(bundle), "bundle_bytes": bundle.stat().st_size,
            "runner_sha256": digest(runner), "arch": "amd64", "os": "Debian 12",
            "test_only": True, "require_complete_tests_and_no_skips": True,
            "container_requirements": {"fresh_namespace": True, "network": "none", "cpus": 2,
                                       "memory_mib": 1536, "memory_swap_mib": 1536, "pids": 512,
                                       "input_read_only": True, "output_private": True},
            "outside_scope": ["official signing", "Release", "new plugin capability acceptance",
                              "historical artifact replacement", "full NodeQuality", "external TCP targets"]}
    with (output / "plan.json").open("x") as destination:
        json.dump(plan, destination, indent=2)
        destination.write("\n")
    print(json.dumps({"status": plan["status"], "output": str(output), "source_commit": args.source_commit,
                      "plan_sha256": digest(output / "plan.json")}))


if __name__ == "__main__":
    main()

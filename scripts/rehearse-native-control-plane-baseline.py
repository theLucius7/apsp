#!/usr/bin/env python3
"""Isolated PostgreSQL 16 rehearsal for scripts/baseline-adoption-native-control-plane-20261117.sql."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CURRENT_SQL = REPO_ROOT / "crates/panel/migrations/0001_current.sql"
FINGERPRINT_SQL = REPO_ROOT / "crates/panel/schema_fingerprint.sql"
ADOPTION_SQL = REPO_ROOT / "scripts/baseline-adoption-native-control-plane-20261117.sql"

PG_IMAGE = "postgres:16"
SOURCE_REV = "8fb798b14761681607973604c0ba15e605e18b4c"
SOURCE_FP = "17b172b9687014f3d65dca438942d14f"

CONTAINER_PREFIX = "apsp-pg16-rehearse-"
OWNER_LABEL = "com.apsp.owner"
OWNER_VALUE = "rehearse-native-control-plane-baseline"
RUN_LABEL = "com.apsp.run_id"

DB_FRESH = "apsp_rehearse_fresh"
DB_SYNTH = "apsp_rehearse_synth"
DB_STRUCT = "apsp_rehearse_struct_copy"

HEX32_RE = re.compile(r"^[0-9a-f]{32}$")
HEX96_RE = re.compile(r"^[0-9a-f]{96}$")
TOC_DATA_RE = re.compile(r"\b(?:TABLE DATA|BLOBS?|LARGE OBJECTS?)\b")

SQLX_DDL = """\
CREATE TABLE public._sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
);
"""

SYNTH_SEED_SQL = """\
SET LOCAL search_path TO public;
INSERT INTO public.singbox_nexus_imports (id, source_id, digest, counts, imported_at, administrator_id)
VALUES ('11111111-1111-4111-8111-111111111111', 'src-1', repeat('a', 64), '{}'::jsonb, 1700000000, 1);
INSERT INTO public.singbox_nexus_bridge (singleton, import_id, subscription_base, enabled, last_sync_at, activated_at)
VALUES (true, '11111111-1111-4111-8111-111111111111', 'https://legacy.example.invalid', true, 1700000123, 1700000050);
INSERT INTO public.servers (id, name, agent_settings, asset_settings)
VALUES (10, 'synth-host', '{}'::jsonb, '{}'::jsonb);
INSERT INTO public.runtime_publications (server_id, manifest_rev, dirty_at)
VALUES (10, 1, NULL);
"""


def clean_env() -> dict[str, str]:
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith("PG") and not k.endswith("DATABASE_URL")
    }
    env["LC_ALL"] = "C"
    return env


def run_cmd(
    cmd: list[str], *, cwd: Path | None = None, input_bytes: bytes | None = None, check: bool = True
) -> subprocess.CompletedProcess[bytes]:
    proc = subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        input=input_bytes,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=clean_env(),
        check=False,
    )
    if check and proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"command failed ({' '.join(cmd[:4])}): {detail or proc.returncode}")
    return proc


def psql(
    container: str, db: str, sql: bytes | str, *, single_tx: bool = True, check: bool = True
) -> subprocess.CompletedProcess[bytes]:
    cmd = [
        "docker", "exec", "-i", container,
        "psql", "-U", "apsp", "-d", db, "-X", "-v", "ON_ERROR_STOP=1", "-q", "-t", "-A",
    ]
    if single_tx:
        cmd.append("-1")
    data = sql.encode("utf-8") if isinstance(sql, str) else sql
    return run_cmd(cmd, input_bytes=data, check=check)


def start_pg16(container: str, run_id: str) -> None:
    run_cmd([
        "docker", "run", "-d", "--network", "none", "--name", container,
        "--label", f"{OWNER_LABEL}={OWNER_VALUE}",
        "--label", f"{RUN_LABEL}={run_id}",
        "-e", "POSTGRES_USER=apsp",
        "-e", f"POSTGRES_PASSWORD={secrets.token_hex(16)}",
        "-e", "POSTGRES_DB=postgres",
        PG_IMAGE,
    ])
    deadline = time.monotonic() + 45.0
    init_done = False
    while time.monotonic() < deadline:
        if not init_done:
            logs = run_cmd(["docker", "logs", container], check=False)
            if b"PostgreSQL init process complete" in (logs.stdout + logs.stderr):
                init_done = True
            else:
                time.sleep(0.4)
                continue
        probe = psql(container, "postgres", "SHOW server_version_num;", single_tx=False, check=False)
        raw = probe.stdout.decode("utf-8", errors="replace").strip()
        if probe.returncode == 0 and raw.isdigit():
            ver = int(raw)
            if not (160000 <= ver < 170000):
                raise RuntimeError(f"expected PostgreSQL 16, got server_version_num={ver}")
            return
        time.sleep(0.4)
    raise RuntimeError(f"timed out waiting for PostgreSQL 16 readiness in {container}")


def cleanup_container(container: str, run_id: str) -> None:
    if not container.startswith(CONTAINER_PREFIX):
        return
    insp = run_cmd([
        "docker", "inspect", "--format",
        f'{{{{index .Config.Labels "{OWNER_LABEL}"}}}}|{{{{index .Config.Labels "{RUN_LABEL}"}}}}',
        container,
    ], check=False)
    if insp.returncode != 0:
        return
    if insp.stdout.decode("utf-8", errors="replace").strip() != f"{OWNER_VALUE}|{run_id}":
        return
    run_cmd(["docker", "rm", "-f", "-v", container], check=False)


def recreate_db(container: str, db: str) -> None:
    psql(container, "postgres", f"DROP DATABASE IF EXISTS {db};", single_tx=False)
    psql(container, "postgres", f"CREATE DATABASE {db};", single_tx=False)


def verify_identity(container: str, db: str, expected_sum: str, fp_query: str) -> str:
    row = psql(
        container, db,
        "SELECT (to_regclass('public._sqlx_migrations_pkey') IS NOT NULL)::text || '|' || "
        "(SELECT count(*)::text FROM public._sqlx_migrations) || '|' || "
        "COALESCE((SELECT version::text || ':' || success::text || ':' || encode(checksum, 'hex') "
        "FROM public._sqlx_migrations LIMIT 1), '') || '|' || "
        "COALESCE((SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton), '');",
    ).stdout.decode("utf-8").strip()
    pkey_ok, mig_cnt, mig_row, saved_fp = row.split("|", 3)
    if pkey_ok != "true" or mig_cnt != "1" or mig_row != f"1:true:{expected_sum}":
        raise RuntimeError(f"{db}: migration identity mismatch ({row})")
    actual_fp = psql(container, db, fp_query).stdout.decode("utf-8").strip()
    if not HEX32_RE.fullmatch(saved_fp) or saved_fp != actual_fp:
        raise RuntimeError(f"{db}: fingerprint mismatch saved={saved_fp} actual={actual_fp}")
    return actual_fp


def init_baseline(
    container: str, db: str, baseline_sql: bytes, expected_sum: str, fp_query: str
) -> str:
    recreate_db(container, db)
    payload = (
        b"SET LOCAL search_path TO public;\n"
        + SQLX_DDL.encode("utf-8") + b"\n"
        + baseline_sql + b"\n"
        + f"INSERT INTO public._sqlx_migrations VALUES (1, 'current', now(), true, decode('{expected_sum}', 'hex'), 1);\n".encode("utf-8")
    )
    psql(container, db, payload, single_tx=True)
    return verify_identity(container, db, expected_sum, fp_query)


def apply_and_verify_rerun(
    container: str, db: str, adoption_sql: bytes, new_sum: str, fresh_fp: str, fp_query: str
) -> None:
    apply_res = psql(container, db, adoption_sql, single_tx=True, check=False)
    if apply_res.returncode != 0:
        err = apply_res.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(
            f"{db}: adoption SQL failed (fresh_fp={fresh_fp}, target_sha384={new_sum}): {err}"
        )
    post_fp = verify_identity(container, db, new_sum, fp_query)
    if post_fp != fresh_fp:
        raise RuntimeError(f"{db}: post-adoption fingerprint {post_fp} != fresh {fresh_fp}")

    rerun = psql(container, db, adoption_sql, single_tx=True, check=False)
    stderr = rerun.stderr.decode("utf-8", errors="replace")
    if (
        rerun.returncode == 0
        or "expected native-lines baseline e96786fb; refusing adoption" not in stderr
    ):
        raise RuntimeError(f"{db}: rerun was not refused by source gate: {stderr.strip()}")
    if verify_identity(container, db, new_sum, fp_query) != fresh_fp:
        raise RuntimeError(f"{db}: state mutated after refused rerun")


def load_structure_copy(archive_path: Path, meta_path: Path) -> tuple[bytes, str]:
    archive = archive_path.read_bytes()
    if archive[:5] != b"PGDMP":
        raise RuntimeError("--structure-archive must be a pg_dump -Fc custom archive")
    meta = json.loads(meta_path.read_text(encoding="utf-8"))
    if not isinstance(meta, dict) or set(meta.keys()) != {"sqlx_migrations", "apsp_schema_baseline"}:
        raise RuntimeError("--structure-meta must contain exact keys sqlx_migrations and apsp_schema_baseline")
    mig = meta["sqlx_migrations"][0] if isinstance(meta["sqlx_migrations"], list) and len(meta["sqlx_migrations"]) == 1 else meta["sqlx_migrations"]
    base = meta["apsp_schema_baseline"][0] if isinstance(meta["apsp_schema_baseline"], list) and len(meta["apsp_schema_baseline"]) == 1 else meta["apsp_schema_baseline"]
    if (
        not isinstance(mig, dict)
        or not isinstance(base, dict)
        or type(mig.get("version")) is not int
        or mig["version"] != 1
        or mig.get("success") is not True
        or not HEX96_RE.fullmatch(str(mig.get("checksum_hex", "")))
        or base.get("singleton") is not True
        or not HEX32_RE.fullmatch(str(base.get("fingerprint", "")))
    ):
        raise RuntimeError("invalid source baseline metadata in --structure-meta")
    meta_sql = (
        "SET LOCAL search_path TO public;\n"
        f"INSERT INTO public._sqlx_migrations VALUES (1, 'current', now(), true, decode('{mig['checksum_hex']}', 'hex'), 1);\n"
        f"INSERT INTO public.apsp_schema_baseline VALUES (true, '{base['fingerprint']}');\n"
    )
    return archive, meta_sql


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Isolated PG16 baseline adoption rehearsal.")
    parser.add_argument("--execute", action="store_true", help="Run isolated PG16 container rehearsal.")
    parser.add_argument("--structure-archive", type=Path, help="Read-only pg_dump -Fc --schema-only file.")
    parser.add_argument("--structure-meta", type=Path, help="Read-only JSON with baseline identity rows.")
    parser.add_argument("--allow-without-structure-copy", action="store_true")
    args = parser.parse_args(argv)

    if not args.execute:
        raise RuntimeError("pass --execute explicitly during unified verification")
    if (args.structure_archive is None) != (args.structure_meta is None):
        raise RuntimeError("--structure-archive and --structure-meta must be passed together")
    if args.structure_archive is None and not args.allow_without_structure_copy:
        raise RuntimeError("provide --structure-archive and --structure-meta, or --allow-without-structure-copy")

    struct_inputs = (
        load_structure_copy(args.structure_archive, args.structure_meta)
        if args.structure_archive is not None
        else None
    )

    current_sql = CURRENT_SQL.read_bytes()
    new_sum = hashlib.sha384(current_sql).hexdigest()
    old_sql = run_cmd(
        ["git", "cat-file", "-p", f"{SOURCE_REV}:crates/panel/migrations/0001_current.sql"],
        cwd=REPO_ROOT,
    ).stdout
    old_sum = hashlib.sha384(old_sql).hexdigest()
    fp_query = "SET LOCAL search_path TO public;\n" + FINGERPRINT_SQL.read_text(encoding="utf-8").strip().rstrip(";") + ";\n"
    adoption_sql = ADOPTION_SQL.read_bytes()

    run_id = f"{os.getpid()}-{secrets.token_hex(4)}"
    container = f"{CONTAINER_PREFIX}{run_id}"
    try:
        start_pg16(container, run_id)
        fresh_fp = init_baseline(container, DB_FRESH, current_sql, new_sum, fp_query)
        synth_fp = init_baseline(container, DB_SYNTH, old_sql, old_sum, fp_query)
        if synth_fp != SOURCE_FP:
            raise RuntimeError(f"unexpected {SOURCE_REV} fingerprint {synth_fp} != {SOURCE_FP}")

        psql(container, DB_SYNTH, SYNTH_SEED_SQL, single_tx=True)
        apply_and_verify_rerun(container, DB_SYNTH, adoption_sql, new_sum, fresh_fp, fp_query)
        bridge = psql(
            container, DB_SYNTH,
            "SELECT native_control_ready::text || '|' || last_sync_at::text || '|' || activated_at::text "
            "FROM public.singbox_nexus_bridge WHERE singleton;",
        ).stdout.decode("utf-8").strip()
        if bridge != "false|1700000123|1700000050":
            raise RuntimeError(f"bridge row mismatch after adoption: {bridge}")

        if struct_inputs is not None:
            archive, meta_sql = struct_inputs
            toc = run_cmd(["docker", "exec", "-i", container, "pg_restore", "-l"], input_bytes=archive).stdout.decode("utf-8", errors="replace")
            if any(TOC_DATA_RE.search(ln) for ln in toc.splitlines() if not ln.lstrip().startswith(";")):
                raise RuntimeError("--structure-archive contains TABLE DATA/BLOB entries; use --schema-only")
            recreate_db(container, DB_STRUCT)
            if re.search(r"\bSCHEMA\s+-\s+public\b", toc):
                psql(container, DB_STRUCT, "DROP SCHEMA IF EXISTS public;", single_tx=True)
            run_cmd([
                "docker", "exec", "-i", container,
                "pg_restore", "-U", "apsp", "-d", DB_STRUCT,
                "--schema-only", "--no-owner", "--no-privileges", "--exit-on-error",
            ], input_bytes=archive)
            psql(container, DB_STRUCT, meta_sql, single_tx=True)
            if verify_identity(container, DB_STRUCT, old_sum, fp_query) != SOURCE_FP:
                raise RuntimeError("structure copy source identity mismatch; refusing adoption")
            apply_and_verify_rerun(container, DB_STRUCT, adoption_sql, new_sum, fresh_fp, fp_query)

        print(json.dumps({
            "fresh_fingerprint": fresh_fp,
            "target_sha384": new_sum,
            "source_fingerprint": synth_fp,
            "source_sha384": old_sum,
            "synthetic": "passed",
            "structure_copy": "passed" if struct_inputs is not None else "not_executed",
            "full_rehearsal_complete": struct_inputs is not None,
        }, indent=2, sort_keys=True))
        return 0
    finally:
        cleanup_container(container, run_id)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Exception as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        sys.exit(1)

"""Read-only SQLite snapshots with explicit columns and private output files."""
import json
import os
from pathlib import Path
import sqlite3


def connect(path):
    path = Path(path).resolve(strict=True)
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA query_only=ON")
    connection.execute("BEGIN")
    return connection


def rows(connection, table, columns, required=True):
    available = {r[1] for r in connection.execute(f'PRAGMA table_info("{table}")')}
    if not available:
        if required:
            raise ValueError(f"Missing source table: {table}")
        return []
    missing = set(columns) - available
    if missing:
        raise ValueError(f"Missing source columns in {table}: {sorted(missing)}")
    selected = ",".join(f'"{c}"' for c in columns)
    result = [dict(row) for row in connection.execute(f'SELECT {selected} FROM "{table}" LIMIT 100001')]
    if len(result) > 100000:
        raise ValueError(f"Source table exceeds the bounded export limit: {table}")
    return result


def write_private(path, value):
    data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(data) > 8 * 1024 * 1024:
        raise ValueError("Snapshot exceeds the 8 MiB import limit")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(data)
    except BaseException:
        Path(path).unlink(missing_ok=True)
        raise

#!/usr/bin/env python3
"""Create a private Compose environment without replacing existing credentials."""

import argparse
import base64
import ipaddress
import json
import os
from pathlib import Path
import secrets
from urllib.parse import urlsplit


def origin(value):
    try:
        parsed = urlsplit(value)
        port = parsed.port
        if (
            parsed.scheme not in ("http", "https")
            or not parsed.hostname
            or parsed.username is not None
            or parsed.password is not None
            or parsed.path not in ("", "/")
            or parsed.query
            or parsed.fragment
            or port == 0
            or any(character.isspace() or character in "'\"\\#$" for character in value)
        ):
            raise ValueError
        host = parsed.hostname
        if ":" in host:
            ipaddress.IPv6Address(host)
            host = f"[{host}]"
        elif any(not character.isascii() or not (character.isalnum() or character in ".-") for character in host):
            raise ValueError
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected an HTTP(S) origin without credentials or path") from error
    return f"{parsed.scheme}://{host}" + (f":{port}" if port is not None else "")


def port_number(value):
    try:
        port = int(value)
        if not 1 <= port <= 65535:
            raise ValueError
    except ValueError as error:
        raise argparse.ArgumentTypeError("port must be between 1 and 65535") from error
    return port


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--public-url", type=origin, required=True)
    parser.add_argument("--port", type=port_number, default=8080)
    parser.add_argument("--output", type=Path, default=Path(".env"))
    parser.add_argument("--release-keys-file", type=Path)
    args = parser.parse_args()
    keys = []
    if args.release_keys_file:
        try:
            keys = json.loads(args.release_keys_file.read_text())
            if not isinstance(keys, list) or not keys or any(
                not isinstance(key, str) or len(base64.b64decode(key, validate=True)) != 42
                or base64.b64decode(key)[:2] != b"Ed" for key in keys
            ):
                raise ValueError("expected minisign public key records")
        except (OSError, ValueError) as error:
            parser.exit(1, f"Cannot read release public keys: {error}\n")
    content = "\n".join([
        "APSP_DB_PASSWORD=" + secrets.token_hex(32),
        "APSP_ADMIN_PASSWORD=" + secrets.token_hex(32),
        "APSP_RELEASE_PUBLIC_KEYS=" + json.dumps(keys, separators=(",", ":")),
        "APSP_PUBLIC_URL=" + args.public_url,
        "APSP_BIND_ADDRESS=127.0.0.1",
        "APSP_PORT=" + str(args.port),
        "RUST_LOG=info",
        "",
    ])
    # O_EXCL also refuses symlinks; permissions apply before any secret is written.
    try:
        descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except OSError as error:
        parser.exit(1, f"Cannot create {args.output}: {error.strerror}\n")
    with os.fdopen(descriptor, "w") as output:
        output.write(content)
    print(f"Created {args.output}; read the administrator password locally from this file.")


if __name__ == "__main__":
    main()

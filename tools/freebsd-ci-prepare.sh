#!/bin/sh
# Install the newest Python packaged for this FreeBSD release with the modules
# the Agent smoke tests need, and expose it under one stable name.
set -eu
pkg update -f
version=$(pkg rquery -g '%n' 'py3*-cryptography' | sed -n 's/^py\(3[0-9]*\)-cryptography$/\1/p' | sort -n | tail -1)
[ -n "$version" ] || { echo 'no packaged Python cryptography module' >&2; exit 1; }
pkg install -y "python$version" "py$version-sqlite3" "py$version-cryptography"
ln -sf "/usr/local/bin/python3.${version#3}" /usr/local/bin/apsp-ci-python
apsp-ci-python -c 'import cryptography, sqlite3, sys; assert sys.version_info >= (3, 11), sys.version'

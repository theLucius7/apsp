#!/usr/bin/env python3
"""Signed index expansion identities and conservative offline-solver budgets.

These functions are internal inputs to the offline IPQuality derivation. The
caller first authenticates the complete parent materials and collection chain.
Release signatures bind the index bytes; they do not promise an APT output
quota, a continuous resource peak, a trusted builder, or a usable rootfs.
"""

import hashlib
import importlib.util
import lzma
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile
import zlib


HERE = Path(__file__).resolve()
SPEC = importlib.util.spec_from_file_location('ipquality_expansion_build',
                                             HERE.with_name('nodequality-rootfs-build.py'))
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)

CHUNK = 65536
XZ_MEMORY_LIMIT = 64 * 1024**2
EXPANDED_COPIES = 3
COMPRESSED_COPIES = 3
# Private configuration, signed Release scratch, bounded command/evidence
# output and directory allocation. The final derivation still has its own
# output admission and continuously polled disk/inode/memory guard.
SOLVER_METADATA_BYTES = 8 * BUILD.MAX_LOCK + 4 * BUILD.MAX_METADATA
EXPANSION_FIELDS = {'uncompressed_path', 'size', 'sha256',
                    'compressed_size', 'compressed_sha256'}


def require(condition, message):
    BUILD.require(condition, message)


def package_path(path, arch):
    require(isinstance(path, str) and path in ('main/binary-' + arch + '/Packages.gz',
                                              'main/binary-' + arch + '/Packages.xz'),
            'unexpected native Packages compressed path')
    return path.rsplit('.', 1)[0]


def stream_expansion(path, compressed, expanded, deadline):
    """Verify one complete gzip/XZ stream without writing its decoded bytes.

Both identities are checked in the same bounded read. Concatenated streams,
padding/trailing bytes, a truncated trailer and a mismatched compression suffix
are refused rather than counted as a successfully authenticated expansion.
"""
    require(isinstance(compressed, dict) and set(compressed) == {'size', 'sha256'},
            'invalid compressed index identity')
    require(isinstance(expanded, dict) and set(expanded) == {'size', 'sha256'},
            'invalid expanded index identity')
    for identity in (compressed, expanded):
        require(type(identity['size']) is int and 0 < identity['size'] <= BUILD.MAX_INDEX
                and isinstance(identity['sha256'], str) and BUILD.SHA256.fullmatch(identity['sha256']),
                'invalid signed index expansion bound')
    path = Path(path)
    require(path.suffix in ('.gz', '.xz'), 'only fixed gzip/xz Packages indices are supported')
    decoder = (zlib.decompressobj(16 + zlib.MAX_WBITS) if path.suffix == '.gz'
               else lzma.LZMADecompressor(format=lzma.FORMAT_XZ, memlimit=XZ_MEMORY_LIMIT))
    encoded_size, decoded_size = 0, 0
    encoded_hash, decoded_hash = hashlib.sha256(), hashlib.sha256()
    stream, before = BUILD.open_regular(path, BUILD.MAX_INDEX)

    def consume(chunk):
        nonlocal decoded_size
        decoded_size += len(chunk)
        require(decoded_size <= expanded['size'], 'expanded Packages exceeds its signed size')
        decoded_hash.update(chunk)

    try:
        with stream:
            require(before.st_size == compressed['size'], 'compressed Packages size differs from signed Release')
            while True:
                deadline.check()
                chunk = stream.read(min(CHUNK, compressed['size'] - encoded_size + 1))
                if not chunk:
                    break
                encoded_size += len(chunk)
                require(encoded_size <= compressed['size'], 'compressed Packages exceeds its signed size')
                encoded_hash.update(chunk)
                require(not decoder.eof, 'trailing bytes after compressed Packages stream')
                pending = chunk
                while True:
                    deadline.check()
                    decoded = decoder.decompress(pending, max_length=CHUNK)
                    consume(decoded)
                    if decoder.eof:
                        require(not decoder.unused_data, 'trailing or concatenated Packages stream')
                        break
                    if path.suffix == '.gz':
                        pending = decoder.unconsumed_tail
                        if not pending and len(decoded) < CHUNK:
                            break
                    else:
                        pending = b''
                        if decoder.needs_input:
                            break
            after = os.fstat(stream.fileno())
            require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                    == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns),
                    'compressed Packages changed during expansion verification')
    except (lzma.LZMAError, zlib.error, EOFError) as error:
        raise ValueError('invalid complete Packages compression stream: ' + str(error)) from error
    require(decoder.eof, 'truncated compressed Packages stream')
    require({'size': encoded_size, 'sha256': encoded_hash.hexdigest()} == compressed,
            'compressed Packages checksum or size differs from signed Release')
    require({'size': decoded_size, 'sha256': decoded_hash.hexdigest()} == expanded,
            'expanded Packages checksum or size differs from signed Release')
    return dict(expanded)


def authenticated_expansion(materials, cache, deadline):
    """Read bounds from actual gpgv cleartext, then verify full decoded indices.

The return value is not a CLI format or an alternative to authenticating the
parent package/source payloads. It is an internal repo/path map for solve().
No existing source inventory or signature receipt format is changed.
"""
    BUILD.validate_materials(materials)
    cache = BUILD.private_directory(cache)
    keyring = BUILD.checked_blob(cache, materials['keyring'], BUILD.MAX_LOCK, deadline)
    scratch_parent = deadline.capacity.output if deadline.capacity is not None else None
    result = {}
    with tempfile.TemporaryDirectory(prefix='apsp-ip-expansion-', dir=scratch_parent) as name:
        scratch = Path(name)
        for repo in materials['repositories']:
            deadline.check()
            inrelease = BUILD.checked_blob(cache, repo['inrelease'], BUILD.MAX_LOCK, deadline)
            release = scratch / (repo['id'] + '.Release')
            status = BUILD.run_bounded(['/usr/bin/gpgv', '--homedir', str(scratch), '--keyring', str(keyring),
                                       '--status-fd', '1', '--output', str(release), str(inrelease)],
                                      BUILD.Deadline(min(30, deadline.remaining()), capacity=deadline.capacity),
                                      CHUNK, stderr=subprocess.DEVNULL)
            BUILD.valid_signers(status, repo['archive'], repo['timestamp'])
            rows = list(BUILD.control_records(BUILD.read_regular(release, BUILD.MAX_LOCK, deadline)))
            require(len(rows) == 1 and rows[0].get('Origin') == 'Debian'
                    and rows[0].get('Codename') == repo['suite'], 'signed expansion Release identity differs')
            sums = BUILD.checksum_rows(rows[0].get('SHA256', ''))
            indices = [row for row in repo['indices'] if row['kind'] == 'Packages']
            require(len(indices) == 1, 'one signed native Packages index per repository is required')
            index = indices[0]
            base = package_path(index['path'], materials['arch'])
            compressed = {key: index[key] for key in ('size', 'sha256')}
            require(sums.get(index['path']) == compressed,
                    'compressed Packages is not covered by authenticated Release')
            require(Path(index['blob']).suffix == Path(index['path']).suffix,
                    'cached Packages compression suffix differs from signed index path')
            expanded = sums.get(base)
            require(expanded is not None, 'signed Release lacks uncompressed Packages Size/SHA256')
            verified = stream_expansion(BUILD.input_path(cache, index['blob']), compressed, expanded, deadline)
            result[repo['id']] = {index['path']: dict(verified, uncompressed_path=base,
                                                     compressed_size=compressed['size'],
                                                     compressed_sha256=compressed['sha256'])}
    expansion_budget(materials['repositories'], result)
    return result


def expansion_budget(repositories, bounds):
    """Validate an internal signed-bound map and budget multiple APT copies.

Three decoded copies cover a retained list and simultaneous temporary/derived
copies; three encoded copies cover mirror/list/temporary files. This is a
conservative admission estimate, not a signature-backed hard APT output quota.
The caller separately checks prospective copies and polls actual resources.
"""
    require(isinstance(repositories, list) and repositories, 'signed repositories are missing')
    require(isinstance(bounds, dict) and set(bounds) == {repo['id'] for repo in repositories},
            'authenticated expansion repository set differs')
    require(len(bounds) == len(repositories), 'duplicate expansion repository identity')
    decoded, encoded, rows = 0, 0, []
    for repo in repositories:
        indices = [row for row in repo['indices'] if row['kind'] == 'Packages']
        require(len(indices) == 1, 'one Packages expansion bound per repository is required')
        index = indices[0]
        mapping = bounds[repo['id']]
        require(isinstance(mapping, dict) and set(mapping) == {index['path']},
                'authenticated expansion index set differs')
        bound = mapping[index['path']]
        require(isinstance(bound, dict) and set(bound) == EXPANSION_FIELDS,
                'invalid internal authenticated expansion fields')
        # The repository row has already passed validate_materials. Bind both
        # its compression suffix and the exact compressed identity again here.
        require(re.fullmatch(r'main/binary-(amd64|arm64)/Packages\.(gz|xz)', index['path'])
                and bound['uncompressed_path'] == index['path'].rsplit('.', 1)[0],
                'authenticated expansion uncompressed path differs')
        require(type(bound['size']) is int and 0 < bound['size'] <= BUILD.MAX_INDEX
                and isinstance(bound['sha256'], str) and BUILD.SHA256.fullmatch(bound['sha256']),
                'invalid authenticated expansion Size/SHA256')
        require(type(bound['compressed_size']) is int and 0 < bound['compressed_size'] <= BUILD.MAX_INDEX
                and bound['compressed_size'] == index['size']
                and isinstance(bound['compressed_sha256'], str)
                and BUILD.SHA256.fullmatch(bound['compressed_sha256'])
                and bound['compressed_sha256'] == index['sha256'],
                'authenticated expansion compressed identity differs')
        decoded += bound['size']
        encoded += bound['compressed_size']
        rows.append(dict(bound, repository=repo['id'], compressed_path=index['path']))
    total = decoded * EXPANDED_COPIES + encoded * COMPRESSED_COPIES + SOLVER_METADATA_BYTES
    return {'schema': 1, 'indices': rows, 'expanded_index_bytes': decoded,
            'compressed_index_bytes': encoded, 'expanded_copies': EXPANDED_COPIES,
            'compressed_copies': COMPRESSED_COPIES, 'metadata_bytes': SOLVER_METADATA_BYTES,
            'reserved_expansion_bytes': total, 'hard_quota': False,
            'accounting_scope': 'signed index identities with conservative APT copy admission; actual resources remain polled'}


def available_memory(meminfo_path=Path('/proc/meminfo'), cgroup_root=Path('/sys/fs/cgroup'),
                     membership_path=Path('/proc/self/cgroup')):
    """Observe host and all visible finite cgroup-v2 ancestor limits.

Raw cgroup headroom is conservative. Inactive file cache and reclaimable slab
are separately recorded as an estimate; reclaim is neither guaranteed nor a
license to weaken the caller's independently imposed resource limit.
"""
    values = {}
    for line in BUILD.read_regular(meminfo_path, CHUNK).decode('ascii').splitlines():
        match = re.fullmatch(r'([A-Za-z0-9_()]+):\s+([0-9]+)(?:\s+(kB))?', line)
        require(match is not None and match[1] not in values, 'invalid or duplicate memory observation')
        values[match[1]] = int(match[2]) * (1024 if match[3] else 1)
    require('MemAvailable' in values, 'host MemAvailable observation is missing')
    host = values['MemAvailable']
    membership = BUILD.read_regular(membership_path, CHUNK).decode('utf-8').splitlines()
    unified = [line[3:] for line in membership if line.startswith('0::')]
    require(len(unified) <= 1, 'ambiguous cgroup-v2 membership')
    observations = []
    if unified:
        relative = PurePosixPath(unified[0])
        require(relative.is_absolute() and '\x00' not in unified[0]
                and all(part not in ('.', '..') for part in unified[0].split('/')),
                'unsafe cgroup-v2 membership')
        root = Path(cgroup_root).resolve(strict=True)
        current = root
        for component in relative.parts[1:]:
            current /= component
            require(current.is_dir() and not current.is_symlink(), 'cgroup observation path is not ordinary')
        require((current / 'memory.max').exists(), 'current cgroup memory controller observation is missing')
        while True:
            require(current.is_dir() and not current.is_symlink(), 'cgroup observation path is not ordinary')
            maximum_path = current / 'memory.max'
            if maximum_path.exists():
                raw_max = BUILD.read_regular(maximum_path, 128).decode('ascii').strip()
                raw_current = BUILD.read_regular(current / 'memory.current', 128).decode('ascii').strip()
                require(raw_current.isdigit() and (raw_max == 'max' or raw_max.isdigit()),
                        'invalid cgroup memory limit observation')
                charged = int(raw_current)
                maximum = None if raw_max == 'max' else int(raw_max)
                stats = {}
                for line in BUILD.read_regular(current / 'memory.stat', CHUNK).decode('ascii').splitlines():
                    parts = line.split()
                    require(len(parts) == 2 and parts[1].isdigit() and parts[0] not in stats,
                            'invalid cgroup memory.stat observation')
                    stats[parts[0]] = int(parts[1])
                events = {}
                for line in BUILD.read_regular(current / 'memory.events', CHUNK).decode('ascii').splitlines():
                    parts = line.split()
                    require(len(parts) == 2 and re.fullmatch(r'[a-z_]+', parts[0])
                            and parts[1].isdigit() and parts[0] not in events,
                            'invalid cgroup memory.events observation')
                    events[parts[0]] = int(parts[1])
                require({'oom', 'oom_kill', 'max'} <= set(events),
                        'cgroup OOM/pressure event observations are missing')
                inactive = stats.get('inactive_file', 0)
                slab = stats.get('slab_reclaimable', 0)
                headroom = None if maximum is None else max(0, maximum - charged)
                reclaim = min(charged, inactive + slab)
                estimate = None if maximum is None else min(maximum, headroom + reclaim)
                observations.append({'path': str(current), 'limit_bytes': maximum,
                                     'current_bytes': charged, 'raw_headroom_bytes': headroom,
                                     'inactive_file_bytes': inactive, 'slab_reclaimable_bytes': slab,
                                     'reclaim_estimate_headroom_bytes': estimate,
                                     'memory_events': events})
            if current == root:
                break
            current = current.parent
    finite = [row for row in observations if row['limit_bytes'] is not None]
    return {'host_available_bytes': host, 'cgroup_scope': 'visible_cgroup_v2_ancestors',
            'cgroup_observations': observations, 'cgroup_v2_observed': bool(unified),
            # A large parent-source hash can charge reclaimable file cache up
            # to memory.max. Raw headroom must remain an observation rather
            # than misrepresenting the host's available memory. The caller
            # separately stops on actual oom/oom_kill event increments.
            'available_bytes': host,
            'cgroup_raw_headroom_bytes': min([row['raw_headroom_bytes'] for row in finite], default=None),
            'reclaim_estimate_available_bytes': min([host] + [row['reclaim_estimate_headroom_bytes'] for row in finite]),
            'reclaim_estimate_is_guaranteed': False}

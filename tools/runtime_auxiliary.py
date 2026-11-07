"""The signed auxiliary-file contract of the current proxy runtime."""
import importlib.util
import io
from pathlib import Path
import tarfile

_spec = importlib.util.spec_from_file_location('apsp_cronet', Path(__file__).parent / 'singbox-runtime' / 'cronet.py')
_cronet = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_cronet)
VERSION = '1.14.4-apsp.2'

def expected(target):
    if target.startswith('windows-'):
        return {'libcronet.dll'}
    if target in ('amd64', 'arm64') or target.startswith('linux-gnu-'):
        return {'libcronet.so'}
    return set()

def inspect(data, target, binary_name):
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as package:
        seen = set()
        for member in package:
            if not member.isfile() or member.name in seen or member.name not in expected(target) | {binary_name}:
                raise ValueError('invalid runtime archive member')
            seen.add(member.name)
            if member.name == binary_name:
                continue
            if member.size > 16000000:
                raise ValueError('oversize runtime library')
            with package.extractfile(member) as source:
                library = source.read()
            platform = 'windows' if target.startswith('windows-') else 'linux'
            _cronet.validate(platform + '_' + target.split('-')[-1], library)
            result[member.name] = library
        if seen != expected(target) | {binary_name}:
            raise ValueError('incomplete runtime artifact')
    return result

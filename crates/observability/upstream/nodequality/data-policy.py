"""Embed exact static reference bytes without executing or downloading data."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
REQUESTS = {
    'ip.sh': {
        b'curl -sL -m 10 "${rawgithub}main/ref/iso3166.json"': 'ip-iso3166.json',
        b'curl $CurlARG -sL "${rawgithub}main/ref/dnsbl.list"': 'ip-dnsbl.list',
    },
    'net.sh': {
        b'curl -sL -m 10 "${rawgithub}main/ref/iso3166.json"': 'net-iso3166.json',
        b'curl -sL -m 10 "${rawgithub}main/ref/province.json"': 'net-province.json',
        b'curl -sL "${rawgithub}main/ref/AS_Mapping.txt"': 'net-AS_Mapping.txt',
        b'curl -sL "${rawgithub}main/ref/iperf.json"': 'net-iperf.json',
        b'curl -sL "${rawgithub}main/ref/speedtest_cn.json"': 'net-speedtest_cn.json',
    },
}
DATA = {'ip-iso3166.json': {'sha256': 'e1434e42786484b1841082a0a16cf27208691443dc6440d125ad81d49007ea42', 'size': 65317}, 'ip-dnsbl.list': {'sha256': 'a92ca482843310167309c82f05cb7e208d7decc7f8bb7eada0f96af42f1143ad', 'size': 8812}, 'net-AS_Mapping.txt': {'sha256': '18596092c50e95ba23a6e997dc1ed823e4b01401034ffa2068a69e3a05c3cc50', 'size': 1154}, 'net-iso3166.json': {'sha256': 'e1434e42786484b1841082a0a16cf27208691443dc6440d125ad81d49007ea42', 'size': 65317}, 'net-province.json': {'sha256': 'a2b8fd700adc16a95dfc9d1671dfca6c1bb0ea3d8f94bf11e8fce2a7c15807e1', 'size': 3960}, 'net-iperf.json': {'sha256': '83170b97007fd9f3792934943d2120cd0c6650b6499ac56c67056f9d20c43298', 'size': 2120}, 'net-speedtest_cn.json': {'sha256': 'ec153fa9b0db16abbb0c93e6eacba6494ce56fc9427d3ea0afe21a9126279d87', 'size': 7772}}
SOURCES = {
    'ip.sh': {'source_sha256': 'f7d7bb8ebce65795f0d54848d9897b5c887cb5eda61a65f5a1df2bf3c43deff9', 'patched_sha256': 'e332b5405ca12fe03ad5f427789c38c92f04bb2b66df05d296060e4cd8483c1e'},
    'net.sh': {'source_sha256': 'b0596e02786b1ae6d1dbedd4af10df6b120135715481dec2a9e75d4eca55cfda', 'patched_sha256': '99f8a26dabdc09163a165edbb9e200f675011d4f7bef5cfe8b0b729b726e861f'},
}


def data_command(content):
    if not isinstance(content, bytes) or not 0 < len(content) <= MAX_SOURCE or b'\0' in content:
        raise ValueError('reference data must be nonempty bounded text without NUL')
    content.decode('utf-8')
    # The static printf format and shell single quotes preserve bytes as data.
    # No eval, format interpolation, external decoder or temporary file is used.
    return b"printf '%s' '" + content.replace(b"'", b"'\\''") + b"'"


def patch(role, content, files):
    if role not in REQUESTS or set(files) != set(REQUESTS[role].values()):
        raise ValueError('unknown static data role or unexpected reference file set')
    for request, name in REQUESTS[role].items():
        if content.count(request) != 1:
            raise ValueError('static data request must occur exactly once')
        content = content.replace(request, data_command(files[name]), 1)
    return content


def transform(role, content, files):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown static data role or source byte limit exceeded')
    if hashlib.sha256(content).hexdigest() != SOURCES[role]['source_sha256']:
        raise ValueError('static data policy input SHA256 mismatch')
    if set(files) != set(REQUESTS[role].values()):
        raise ValueError('static data file set mismatch')
    for name, data in files.items():
        expected = DATA[name]
        if (not isinstance(data, bytes) or len(data) != expected['size']
                or hashlib.sha256(data).hexdigest() != expected['sha256']):
            raise ValueError('static data identity mismatch: ' + name)
    result = patch(role, content, files)
    if len(result) > MAX_SOURCE or hashlib.sha256(result).hexdigest() != SOURCES[role]['patched_sha256']:
        raise ValueError('static data output SHA256 or byte limit mismatch')
    return result

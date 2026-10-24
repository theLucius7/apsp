#!/usr/bin/env python3
"""Remove swap side effects from two exact pinned sources, without executing them."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
ENTRY_REPLACEMENTS = [
    (b'    swapoff $work_dir/swap 2>/dev/null\n',
     b'    : # Apsp modification (2026-10-01): never change host swap.\n'),
    (b'    . <(curl -sL "$raw_file_prefix/part/swap.sh")\n',
     b'    : # Apsp modification (2026-10-01): do not load the unused swap helper.\n'),
    (b'        run_HardwareQuality | tee $result_directory/$hardware_quality_filename\n',
     b'        (set -o pipefail; run_HardwareQuality | tee $result_directory/$hardware_quality_filename) || exit $? # Apsp: stop after hardware refusal.\n'),
]
HARDWARE_PREFIX = rb'''test_cpu_gb5(){
local mem_avail_mb swap_free_mb
mem_avail_mb=$(awk '/MemAvailable:/ {print int($2/1024)}' /proc/meminfo)
swap_free_mb=$(awk '/SwapFree:/ {print int($2/1024)}' /proc/meminfo)
[[ $mem_avail_mb =~ ^[0-9]+$ ]]||mem_avail_mb=0
[[ $swap_free_mb =~ ^[0-9]+$ ]]||swap_free_mb=0
local need_swap=0
local swap_file=""
local target_total_mb=1200
if ((mem_avail_mb<950));then
if [[ ${osinfo[virt_kind]} == "container" || ${osinfo[virt_kind]} == "unknown" ]];then
return
fi
if ((mem_avail_mb+swap_free_mb<target_total_mb));then
local create_mb=$((target_total_mb-mem_avail_mb-swap_free_mb))
((create_mb<128))&&create_mb=128
local avail_disk_mb
avail_disk_mb=$(df -Pm "$workdir" 2>/dev/null|awk 'NR==2 {print $4}')
if ! [[ $avail_disk_mb =~ ^[0-9]+$ ]]||((avail_disk_mb<create_mb+100));then
return
fi
swap_file="$workdir/.gb5_tmp.swap"
if fallocate -l "${create_mb}M" "$swap_file" 2>/dev/null||dd if=/dev/zero of="$swap_file" bs=1M count="$create_mb" status=none;then
chmod 600 "$swap_file"&&mkswap "$swap_file" >/dev/null 2>&1&&swapon "$swap_file" >/dev/null 2>&1&&need_swap=1
else
return
fi
fi
fi
'''
MEMORY_GUARD = rb'''test_cpu_gb5(){
# Apsp modification (2026-10-01): refuse low memory without creating swap.
# Preserve the upstream 950 MiB host-memory guard; this is not a GB5 budget.
local mem_avail_mb
mem_avail_mb=$(awk '/MemAvailable:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null)
if ! [[ $mem_avail_mb =~ ^[0-9]+$ ]] || ((mem_avail_mb<950));then
printf '%s\n' 'Error: insufficient available memory for Geekbench 5 (requires at least 950 MiB); automatic swap is disabled' >&2
exit 70
fi
'''
SWAP_CLEANUP = rb'''[[ $need_swap -eq 1 ]]&&{
swapoff "$swap_file" 2>/dev/null
[[ -n $swap_file ]]&&rm -f "$swap_file"
}
'''
NO_SWAP_CLEANUP = rb'''# Apsp: no swap was created, so cleanup must not change host swap.
'''
SOURCES = {
    'NodeQuality.sh': {
        'source_sha256': '4e1b25894cadf908ef61fb0d9ce874a75524c6dafc2ea26f0477107288e0c018',
        'patched_sha256': 'f7355a4ca5f4042a535e7a73ac337dc427e8f6ca5bdd7e9fb96181fc3162d91b',
    },
    'hardware.sh': {
        'source_sha256': 'f7413e8a19eaacce2df70b1ae6c63bab89334bca0d07846badacde5ef6afcb0c',
        'patched_sha256': 'e1f90cb9eaf80098a48beb008187b422cb664fc32cf2a4ee306b398ee473567c',
    },
}


def replace_once(content, before, after):
    if content.count(before) != 1:
        raise ValueError('exact swap policy anchor must occur exactly once')
    return content.replace(before, after, 1)


def patch(role, content):
    if role == 'NodeQuality.sh':
        for before, after in ENTRY_REPLACEMENTS:
            content = replace_once(content, before, after)
    elif role == 'hardware.sh':
        content = replace_once(content, HARDWARE_PREFIX, MEMORY_GUARD)
        content = replace_once(content, SWAP_CLEANUP, NO_SWAP_CLEANUP)
    else:
        raise ValueError('unknown fixed swap policy role')
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown swap role or source byte limit exceeded')
    spec = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != spec['source_sha256']:
        raise ValueError('swap policy input SHA256 mismatch')
    result = patch(role, content)
    if len(result) > MAX_SOURCE + 2048 or hashlib.sha256(result).hexdigest() != spec['patched_sha256']:
        raise ValueError('swap policy output SHA256 or byte limit mismatch')
    return result

#!/usr/bin/env python3
"""Verify native services and real runtime reconciliation on disposable CI hosts only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import runpy
import shutil
import socket
import subprocess
import sys
import time
import uuid

helpers = runpy.run_path(str(Path(__file__).with_name('agent-smoke.py')))
Panel, invoke, wait_for, status = (helpers[name] for name in ('Panel', 'invoke', 'wait_for', 'status'))
SYSTEM = platform.system()
RELEASE = helpers['RELEASE']


def command(args, check=True):
    result = subprocess.run(args, capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=90)
    if check and result.returncode:
        raise RuntimeError(f'{args[0]} failed: {result.stdout}\n{result.stderr}')
    return result


def powershell(script, check=True):
    return command(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', "$ErrorActionPreference='Stop'; " + script], check)


def windows_rights(root):
    path = root / 'user-rights.inf'
    command(['secedit.exe', '/export', '/cfg', str(path), '/areas', 'USER_RIGHTS', '/quiet'])
    rights = {}
    for line in path.read_text(encoding='utf-16').splitlines():
        name, separator, value = line.partition('=')
        if separator and name.strip().startswith('Se'):
            rights[name.strip()] = {member.strip() for member in value.split(',') if member.strip()}
    path.unlink()
    names = sorted({member for members in rights.values() for member in members if not member.startswith('*')})
    resolved = {}
    if names:
        literals = ','.join("'" + name.replace("'", "''") + "'" for name in names)
        output = powershell("$sids=@{}; foreach($name in @(" + literals + ")) { $sids[$name]=([Security.Principal.NTAccount]::new($name)).Translate([Security.Principal.SecurityIdentifier]).Value }; $sids | ConvertTo-Json -Compress")
        resolved = json.loads(output.stdout)
    return {name: {member[1:] if member.startswith('*') else resolved[member] for member in members}
            for name, members in rights.items()}


def assert_windows_rights(root, expected):
    actual = windows_rights(root)
    changes = {name: dict(expected=sorted(expected.get(name, set())), actual=sorted(actual.get(name, set())))
               for name in expected.keys() | actual.keys() if expected.get(name, set()) != actual.get(name, set())}
    assert not changes, 'Unexpected user rights changes: ' + json.dumps(changes)


def service(name, action):
    if SYSTEM == 'Darwin':
        label = 'system/org.apsp.' + name.replace('@', '.')
        args = ['launchctl', 'kickstart', '-k', label] if action == 'restart' else ['launchctl', 'bootout', label]
    elif SYSTEM == 'FreeBSD':
        args = ['service', name.replace('-', '_').replace('@', '_'), action]
    else:
        script = f"Stop-ScheduledTask -TaskName '{name}'"
        if action == 'restart':
            script += f"; Start-Sleep -Seconds 1; Start-ScheduledTask -TaskName '{name}'"
        powershell(script)
        return
    command(args)


def port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def transfer(port_number):
    # Plain loopback VLESS is only a transport fixture; production uses Reality.
    with socket.socket() as echo:
        echo.settimeout(5)
        echo.bind(('127.0.0.1', 0))
        echo.listen()
        with socket.create_connection(('127.0.0.1', port_number), timeout=5) as proxy:
            body = bytes([42]) * 1024
            proxy.sendall(b'\0' + uuid.UUID(int=1).bytes + b'\0\1' +
                          echo.getsockname()[1].to_bytes(2, 'big') + b'\1\x7f\0\0\1' + body)
            stream, _ = echo.accept()
            with stream:
                received = bytearray()
                while len(received) < len(body):
                    received.extend(stream.recv(len(body) - len(received)))
                assert bytes(received) == body
                stream.sendall(body)
            received = bytearray()
            while len(received) < 1026:
                chunk = proxy.recv(1026 - len(received))
                assert chunk
                received.extend(chunk)
            assert received == b'\0\0' + body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('runtime_archive', type=Path)
    args = parser.parse_args()
    if os.environ.get('CI') != 'true' or SYSTEM not in ('Darwin', 'FreeBSD', 'Windows'):
        parser.error('requires a disposable native CI runner')
    binary = args.binary.resolve()
    archive = args.runtime_archive.read_bytes()
    version = subprocess.check_output([str(binary), '--version'], text=True).strip().split()[-1]
    root = (Path(os.environ['ProgramData']) if SYSTEM == 'Windows' else Path('/opt')) / ('apsp-test-' + uuid.uuid4().hex[:8])
    root.mkdir(mode=0o755, parents=True)
    source = root / 'verified-source' / version
    source.mkdir(mode=0o755, parents=True)
    signed_binary = source / binary.name
    shutil.copy2(binary, signed_binary)
    binary = signed_binary
    RELEASE['install'](source, RELEASE['proof']('agent', version, binary.name, binary.read_bytes()))
    os.environ['NO_PROXY'] = os.environ['no_proxy'] = '127.0.0.1,localhost'
    panel = Panel()
    config = helpers['configure'](root, panel.origin)
    # Native permission updates and service operations involve multiple OS commands.
    config.write_text(config.read_text().replace('operation_timeout_secs = 5', 'operation_timeout_secs = 120').replace(json.dumps(str(root / 'state.db')), json.dumps(str(root / 'state/core/state.db'))).replace(json.dumps(str(root / 'status.sock')), json.dumps(str(root / 'state/core/status.sock'))))
    proxy_port, stats_port = port(), port()
    native = dict(log=dict(level='error'), inbounds=[dict(type='vless', tag='fixture', listen='127.0.0.1',
                  listen_port=proxy_port, users=[dict(name='u1_n1', uuid=str(uuid.UUID(int=1)))])],
                  outbounds=[dict(type='direct', tag='direct')], route=dict(final='direct'),
                  experimental=dict(v2ray_api=dict(listen=f'127.0.0.1:{stats_port}', stats=dict(enabled=True, users=['u1_n1']))))
    bundle = json.dumps(dict(files={'config.json': json.dumps(native)}), separators=(',', ':')).encode()
    runtime_path = '/api/agent/v1/artifacts/sing-box/1.14.2/' + RELEASE['target']()
    panel.downloads = {runtime_path: archive, '/fixture/bundle': bundle}
    module = dict(kernel_version='1.14.2', artifact=dict(url=panel.origin + runtime_path, sha256=hashlib.sha256(archive).hexdigest(),
                  proof=RELEASE['proof']('sing-box', '1.14.2', 'sing-box.exe' if SYSTEM == 'Windows' else 'sing-box', archive, 'tar.gz')),
                  config_rev=1, bundle_url=panel.origin + '/fixture/bundle', bundle_sha256=hashlib.sha256(bundle).hexdigest(), stats_listen=f'127.0.0.1:{stats_port}')
    try:
        if SYSTEM == 'Windows':
            command(['wevtutil.exe', 'sl', 'Microsoft-Windows-TaskScheduler/Operational', '/e:true'])
            original_rights = windows_rights(root)
        print(invoke(binary, config, 'enroll', '--panel', panel.origin, '--token', 'smoke-enrollment'))
        # Publish after services are registered so launchd's eager startup cannot race the fixture.
        print(invoke(binary, config, 'install-service'))
        if SYSTEM == 'Windows':
            account = powershell("$user=Get-LocalUser -Name 'apsp-singbox'; if (-not (Get-LocalGroupMember -SID 'S-1-5-32-545' | Where-Object { $_.SID -eq $user.SID })) { throw 'Runtime is not an ordinary Users member' }; if (Get-LocalGroupMember -SID 'S-1-5-32-544' | Where-Object { $_.SID -eq $user.SID }) { throw 'Runtime must not be an administrator' }; $user.SID.Value").stdout.strip()
            expected_rights = dict(original_rights)
            expected_rights['SeBatchLogonRight'] = original_rights.get('SeBatchLogonRight', set()) | {account}
            assert_windows_rights(root, expected_rights)
            # Capture fixture runtime output without changing its account or launch arguments.
            launcher = root / 'core/runtime-launcher.ps1'
            transcript = str(root / 'runtime/sing-box@main/data/runtime-transcript.log').replace("'", "''")
            prefix = "Start-Transcript -LiteralPath '" + transcript + "' -Append | Out-Null\n"
            launcher.write_bytes(prefix.encode('utf-8') + launcher.read_bytes())
        panel.manifest = dict(rev=1, modules={'singbox': module})
        revision = 1
        def applied():
            info = status(binary, config)
            return info and info['healthy'].get('singbox') and info['applied'].get('singbox') == revision
        wait_for(applied, 'native runtime reconciliation', 180)
        transfer(proxy_port)
        previous_port = proxy_port
        proxy_port = port()
        native['inbounds'][0]['listen_port'] = proxy_port
        bundle = json.dumps(dict(files={'config.json': json.dumps(native)}), separators=(',', ':')).encode()
        panel.downloads['/fixture/bundle-2'] = bundle
        revision = 2
        module = dict(module, config_rev=revision, bundle_url=panel.origin + '/fixture/bundle-2',
                      bundle_sha256=hashlib.sha256(bundle).hexdigest())
        panel.manifest = dict(rev=revision, modules={'singbox': module})
        wait_for(applied, 'native runtime configuration reload', 180)
        transfer(proxy_port)
        with socket.socket() as closed:
            closed.settimeout(5)
            assert closed.connect_ex(('127.0.0.1', previous_port)) != 0, 'old runtime listener survived reload'
        old = status(binary, config)['pid']
        service('apsp-agent', 'restart')
        wait_for(lambda: (s := status(binary, config)) and s['pid'] != old and s['connected'], 'Agent service restart', 90)
        transfer(proxy_port)
        # Reinstalling must preserve identity, service independence and immutable artifacts.
        identity = (root / 'identity/device.key').read_bytes()
        print(invoke(binary, config, 'install-service'))
        assert (root / 'identity/device.key').read_bytes() == identity
        if SYSTEM == 'Windows':
            assert_windows_rights(root, expected_rights)
        transfer(proxy_port)
        service('apsp-agent', 'stop')
        wait_for(lambda: status(binary, config) is None, 'Agent service shutdown', 30)
        transfer(proxy_port)
        if SYSTEM == 'Darwin':
            data = command(['launchctl', 'print', 'system/org.apsp.apsp-singbox.main']).stdout
            assert 'apsp-singbox' in data
        elif SYSTEM == 'FreeBSD':
            assert command(['sysrc', '-n', 'apsp_agent_enable']).stdout.strip() == 'YES'
        else:
            powershell("$t=Get-ScheduledTask -TaskName 'apsp-singbox@main'; if ($t.Principal.UserId -notmatch 'apsp-singbox') { throw 'Runtime account mismatch' }; if ($t.Triggers.Count -ne 1) { throw 'Missing startup trigger' }")
        # The independent OS service must recheck cached proof without a running
        # Agent worker. A valid initial installation must not authorize later bytes.
        proof_path = root / 'plugins/sing-box/1.14.2/release.json'
        original_proof = proof_path.read_bytes()
        def listening():
            with socket.socket() as connection:
                connection.settimeout(1)
                return connection.connect_ex(('127.0.0.1', proxy_port)) == 0
        try:
            proof_path.write_bytes(original_proof + b' ')
            try:
                service('apsp-singbox@main', 'restart')
            except RuntimeError:
                # Some service managers report the expected startup rejection.
                pass
            wait_for(lambda: not listening(), 'runtime rejects corrupted cached proof', 30)
            for _ in range(6):
                time.sleep(0.5)
                assert not listening(), 'native runtime restarted without verifying its signed proof'
        finally:
            proof_path.write_bytes(original_proof)
        service('apsp-singbox@main', 'restart')
        wait_for(listening, 'runtime accepts restored signed proof', 90)
        transfer(proxy_port)
        print('Native services: startup registration, privilege separation, runtime artifact/configuration, reload, traffic, Agent restart/reinstall, independent runtime and cache signature rejection passed')
    except BaseException as error:
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stdout, error.stderr)
        for path in [root / 'core/update-state.json', Path('/var/log/apsp-agent.log'), Path('/var/log/apsp-singbox@main.log')]:
            if path.exists():
                print(path, path.read_text(errors='replace')[-12000:])
        if SYSTEM == 'Darwin':
            print(command(['launchctl', 'print', 'system/org.apsp.apsp-singbox.main'], False).stdout)
            print(command(['id', 'apsp-singbox'], False).stdout)
            print(command(['ls', '-lde', str(root), str(root / 'runtime'), str(root / 'runtime/sing-box@main/data'), str(root / 'core/runtime-launcher.sh')], False).stdout)
        if SYSTEM == 'FreeBSD':
            print(command(['service', 'apsp_agent', 'onestatus'], False).stdout)
            print(command(['service', 'apsp_singbox_main', 'onestatus'], False).stdout)
            messages = Path('/var/log/messages')
            if messages.exists():
                print(messages.read_text(errors='replace')[-6000:])
        if SYSTEM == 'Windows':
            print(powershell("Get-ScheduledTaskInfo -TaskName 'apsp-agent'; Get-ScheduledTaskInfo -TaskName 'apsp-singbox@main'", False).stdout)
            print(powershell("$scheduler=New-Object -ComObject Schedule.Service; $scheduler.Connect(); $task=$scheduler.GetFolder('\\').GetTask('apsp-singbox@main'); $task.GetSecurityDescriptor(7); Get-LocalUser -Name 'apsp-singbox'; Get-LocalGroupMember -SID 'S-1-5-32-545'; Get-WinEvent -LogName 'Microsoft-Windows-TaskScheduler/Operational' -MaxEvents 100 | Where-Object { $_.Message -match 'apsp-' } | Select-Object -First 6 TimeCreated,Id,Message | ConvertTo-Json -Compress", False).stdout)
            print(powershell("Get-CimInstance Win32_Process -Filter \"Name='apsp-agent.exe' OR Name='sing-box.exe'\" | Select-Object ProcessId,ParentProcessId,CreationDate,CommandLine | ConvertTo-Json -Compress", False).stdout)
            transcript = root / 'runtime/sing-box@main/data/runtime-transcript.log'
            if transcript.exists():
                print('Runtime transcript:', transcript.read_text(encoding='utf-8-sig', errors='replace')[-8000:])
        # Keep the application failure after verbose service diagnostics in the bounded CI tail.
        print('Apply results:', json.dumps([m for m in panel.messages if m['type'] == 'apply.result'][-8:]))
        try:
            print('Current Agent status:', json.dumps(status(binary, config)))
        except (OSError, subprocess.SubprocessError) as status_error:
            print('Agent status unavailable:', status_error)
        raise
    finally:
        for name in ('apsp-agent', 'apsp-singbox@main'):
            try:
                service(name, 'stop')
            except (RuntimeError, subprocess.TimeoutExpired):
                pass
            if SYSTEM == 'Windows':
                powershell(f"Unregister-ScheduledTask -TaskName '{name}' -Confirm:$false", False)
            elif SYSTEM == 'Darwin':
                Path('/Library/LaunchDaemons', 'org.apsp.' + name.replace('@', '.') + '.plist').unlink(missing_ok=True)
            else:
                Path('/usr/local/etc/rc.d', name.replace('-', '_').replace('@', '_')).unlink(missing_ok=True)
        panel.shutdown()
        panel.server_close()
        shutil.rmtree(root)


if __name__ == '__main__':
    main()

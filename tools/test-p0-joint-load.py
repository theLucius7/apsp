#!/usr/bin/env python3
"""Check that joint acceptance cannot substitute metrics or erase failures."""
import copy
import gzip
import json
from pathlib import Path
import runpy
import subprocess
import sys
import threading
import unittest
from unittest import mock
import urllib.error
import urllib.request

MODULE = runpy.run_path(str(Path(__file__).with_name('p0-joint-load.py')))


def ledger():
    value = MODULE['Ledger']()
    value.heartbeats = [dict(phase='baseline', session=1, monotonic=t) for t in [0, 20, 40]]
    value.heartbeats.extend([dict(phase='agent_restart', session=2, monotonic=50),
                             dict(phase='panel_disconnect', session=3, monotonic=80),
                             dict(phase='recovery', session=3, monotonic=100)])
    value.events = [dict(phase=phase, event='ws_open', session=identifier) for identifier, phase in
                    [(1, 'prepare'), (2, 'agent_restart'), (3, 'panel_disconnect')]]
    value.samples = [dict(id=str(i), sampled_at_ms=1000 + i) for i in range(3)]
    value.traffic = [dict(phase=phase, ok=True) for phase in
                     ['baseline', 'diagnostic_fixtures', 'agent_restart', 'panel_disconnect', 'recovery']]
    return value


class JointAcceptanceTests(unittest.TestCase):
    def test_actual_three_periods_are_required(self):
        value = ledger()
        self.assertEqual(MODULE['evaluate'](value)['baseline_heartbeat_count'], 3)
        value.heartbeats[1]['monotonic'], value.heartbeats[2]['monotonic'] = 1, 2
        with self.assertRaises(AssertionError):
            MODULE['evaluate'](value)

    def test_frequent_metrics_cannot_hide_heartbeat_gap(self):
        value = ledger()
        value.heartbeats[1]['monotonic'], value.heartbeats[2]['monotonic'] = 31, 51
        value.samples *= 100
        with self.assertRaisesRegex(AssertionError, 'heartbeat interval'):
            MODULE['evaluate'](value)

    def test_explicit_outage_global_gap_stays_in_report(self):
        value = ledger()
        value.heartbeats[4]['monotonic'], value.heartbeats[5]['monotonic'] = 100, 120
        result = MODULE['evaluate'](value)
        self.assertEqual(result['max_connected_session_heartbeat_gap_seconds'], 20)
        self.assertIn(50, result['global_heartbeat_gaps_seconds'])

    def test_proxy_failure_is_preserved(self):
        value = ledger()
        value.traffic[2]['ok'] = False
        with self.assertRaisesRegex(AssertionError, 'proxy echo failed'):
            MODULE['evaluate'](value)

    def test_worker_launch_failure_cannot_hide_after_successful_recovery(self):
        value = ledger()
        value.change('recovery')
        stop = mock.Mock()
        stop.is_set.side_effect = [False, True]
        with mock.patch.object(subprocess, 'run', side_effect=OSError('bounded launch failure')):
            MODULE['traffic_worker'](value, stop, 2080)
        self.assertFalse(value.traffic[-1]['ok'])
        self.assertEqual(value.traffic[-1]['error_type'], 'OSError')
        self.assertEqual(value.traffic[-1]['phase'], 'recovery')
        with self.assertRaisesRegex(AssertionError, 'proxy echo failed'):
            MODULE['evaluate'](value)

    def test_inflight_failure_during_stop_is_in_final_evaluation(self):
        value = ledger()
        value.change('recovery')
        stop, entered = threading.Event(), threading.Event()
        def last_transfer(port):
            entered.set()
            if not stop.wait(2):
                raise TimeoutError('test worker was not stopped')
            return dict(ok=False, exit_code=1)
        function = MODULE['traffic_worker']
        with mock.patch.dict(function.__globals__, transfer_guard=last_transfer):
            worker = threading.Thread(target=function, args=(value, stop, 2080))
            worker.start()
            try:
                self.assertTrue(entered.wait(2))
                MODULE['evaluate'](value)  # The pre-stop snapshot still passes.
            finally:
                stop.set()
                worker.join(timeout=3)
            self.assertFalse(worker.is_alive())
        with self.assertRaisesRegex(AssertionError, 'proxy echo failed'):
            MODULE['evaluate'](value)

    def test_unexpected_reconnect_cannot_hide_a_gap(self):
        value = ledger()
        value.heartbeats[2]['session'] = 2
        with self.assertRaisesRegex(AssertionError, 'unexpected reconnect'):
            MODULE['evaluate'](value)

    def test_optimized_worker_is_rejected_before_network(self):
        result = subprocess.run([sys.executable, '-O', str(Path(__file__).with_name('p0-joint-load.py')),
                                 '--echo-worker', '2080'], capture_output=True, text=True, timeout=3)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('assertion checks must remain enabled', result.stderr)

    def test_replayed_id_cannot_acquire_a_new_sampling_time(self):
        value = ledger()
        value.samples.append(dict(id='0', sampled_at_ms=9000))
        with self.assertRaisesRegex(AssertionError, 'replay changed'):
            MODULE['evaluate'](value)

    def test_host_forwarding_receipt_must_match_this_boot(self):
        receipt = dict(postcheck=dict(boot_id_sha256='current', host_shared_fs_types=[], guest_probe_listeners_left=0),
                       management_loopback_listener_verified=True, guest_probe_exited=True, ready_file_removed=True,
                       observations=[dict(checks=[dict(host_connect_errno=61, host_listener=False)] * 2)])
        MODULE['verify_isolation'](receipt, 'current')
        with self.assertRaises(AssertionError):
            MODULE['verify_isolation'](receipt, 'older')
        bad = copy.deepcopy(receipt)
        bad['observations'][0]['checks'][0]['host_listener'] = True
        with self.assertRaises(AssertionError):
            MODULE['verify_isolation'](bad, 'current')

    def test_recovery_requires_two_beats_on_one_session(self):
        value = ledger()
        value.heartbeats.pop(4)
        with self.assertRaisesRegex(AssertionError, 'two actual heartbeats'):
            MODULE['evaluate'](value)
        value = ledger()
        value.heartbeats[-1]['monotonic'] = 81
        with self.assertRaisesRegex(AssertionError, 'recovered heartbeat period'):
            MODULE['evaluate'](value)
        value = ledger()
        value.events.append(dict(event='ws_open', phase='recovery', session=4))
        with self.assertRaisesRegex(AssertionError, 'unplanned reconnect'):
            MODULE['evaluate'](value)

    def test_real_503_records_timestamp_before_ack_and_rejects_changed_replay(self):
        value = MODULE['Ledger']()
        panel = MODULE['Panel'](value)
        try:
            def send(stamp):
                data = gzip.compress(json.dumps(dict(samples=[dict(id='sample', sampled_at=stamp)])).encode())
                request = urllib.request.Request(panel.origin + '/api/agent/v1/telemetry', data=data,
                    headers={'Content-Encoding': 'gzip', 'Authorization': 'Bearer ' + MODULE['SMOKE']['SESSION']})
                return urllib.request.urlopen(request, timeout=2)
            panel.acknowledge = False
            with self.assertRaises(urllib.error.HTTPError) as failure:
                send(1000)
            self.assertEqual(failure.exception.code, 503)
            failure.exception.close()
            self.assertEqual(panel.seen, {'sample'})
            self.assertEqual(panel.samples, {})
            self.assertEqual(value.attempts[0]['sampled_at_ms'], 1000)
            self.assertFalse(value.attempts[0]['acknowledged'])
            panel.acknowledge = True
            with send(1001) as response:
                self.assertEqual(json.load(response), {'ids': ['sample']})
            with self.assertRaisesRegex(AssertionError, 'pre-ACK timestamp changed'):
                MODULE['verify_sampling_attempts'](value)
        finally:
            panel.disconnect(); panel.shutdown(); panel.server_close()

    def test_unknown_or_global_oom_cannot_pass_as_empty_set(self):
        for kernel in ['python invoked oom-killer: unknown', 'Killed process 42 (python3)',
                       'oom-kill:constraint=CONSTRAINT_NONE,task=python3,pid=42,uid=0']:
            with self.subTest(kernel=kernel):
                result = MODULE['classify_oom'](kernel)
                self.assertFalse(result['allowed'])
                self.assertEqual(result['status'], 'unknown_or_disallowed')
                self.assertEqual(result['lines'], [kernel])

    def test_only_complete_identity_matched_diagnostic_oom_is_allowed(self):
        unit = 'apsp-diagnostic-11111111-1111-4111-8111-111111111111.service'
        kernel = ('python3 invoked oom-killer: oom_score_adj=500\n'
                  'oom_kill_process+0x2ec/0x2f0\n'
                  f'oom-kill:constraint=CONSTRAINT_MEMCG,oom_memcg=/system.slice/{unit},task_memcg=/system.slice/{unit},task=python3,pid=42,uid=0\n'
                  'Memory cgroup out of memory: Killed process 42 (python3) oom_score_adj:500\n')
        self.assertTrue(MODULE['classify_oom'](kernel)['allowed'])
        self.assertFalse(MODULE['classify_oom'](kernel.replace('Killed process 42', 'Killed process 43'))['allowed'])
        self.assertFalse(MODULE['classify_oom'](kernel + 'unattributed out of memory')['allowed'])

    def test_cleanup_errors_are_recorded_and_next_unit_can_be_checked(self):
        function = MODULE['cleanup_unit']
        with mock.patch.object(function.__globals__['subprocess'], 'run', side_effect=subprocess.TimeoutExpired('systemctl', 10)), \
             mock.patch.dict(function.__globals__, properties=lambda unit: dict(MainPID='9', ControlPID='0')):
            failed = function('fixture-one.service')
        self.assertFalse(failed['clean'])
        self.assertEqual(failed['stop_error'], 'TimeoutExpired')
        with mock.patch.object(function.__globals__['subprocess'], 'run', return_value=subprocess.CompletedProcess([], 0)), \
             mock.patch.dict(function.__globals__, properties=lambda unit: dict(MainPID='0', ControlPID='0')):
            following = function('fixture-two.service')
        self.assertTrue(following['clean'])

    def test_zero_pids_cannot_hide_cgroup_read_failure(self):
        function = MODULE['cleanup_unit']
        processes = mock.MagicMock()
        processes.read_text.side_effect = OSError('bounded fixture read failure')
        cgroup = mock.MagicMock()
        cgroup.__truediv__.return_value = cgroup
        cgroup.exists.return_value = True
        cgroup.rglob.return_value = [processes]
        status = lambda unit: dict(MainPID='0', ControlPID='0', ControlGroup='/fixture')
        with mock.patch.object(function.__globals__['subprocess'], 'run', return_value=subprocess.CompletedProcess([], 0)), \
             mock.patch.dict(function.__globals__, properties=status, Path=mock.MagicMock(return_value=cgroup)):
            result = function('fixture.service')
        self.assertFalse(result['clean'])
        self.assertEqual(result['confirm_error'], 'OSError')


if __name__ == '__main__':
    unittest.main()

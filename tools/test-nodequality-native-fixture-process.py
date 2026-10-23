#!/usr/bin/env python3
"""Portable ownership contracts; fake children never execute native programs."""
import json
from pathlib import Path
import signal
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

import nodequality_native_fixture_process as fixture


class FixtureOwnershipContracts(unittest.TestCase):
    def test_missing_nonreaping_interface_refuses_before_spawn(self):
        with mock.patch.object(fixture, 'hasattr', return_value=False, create=True), \
                mock.patch.object(subprocess.Popen, '__init__', return_value=None) as spawn:
            with self.assertRaisesRegex(ValueError, 'fixture_nonreaping_process_observation_required'):
                fixture.ObservedProcess(['TEST_ONLY never executed'], start_new_session=True)
        spawn.assert_not_called()

    def test_live_group_permission_error_is_retained_but_zombie_only_group_can_exit(self):
        for alive in (True, False):
            with self.subTest(live_after_error=alive), \
                    mock.patch.object(fixture, 'group_has_live_members', side_effect=[True, alive]), \
                    mock.patch.object(fixture.os, 'killpg', side_effect=PermissionError('TEST_ONLY')) as signal_group:
                if alive:
                    with self.assertRaises(PermissionError):
                        fixture.signal_group_if_live(9876, signal.SIGKILL)
                else:
                    fixture.signal_group_if_live(9876, signal.SIGKILL)
                signal_group.assert_called_once_with(9876, signal.SIGKILL)

    def test_nonfinite_or_unbounded_deadlines_refuse_without_observing_a_child(self):
        process = object.__new__(fixture.ObservedProcess)
        owner = object.__new__(fixture.OwnedProcess)
        for timeout in (None, True, float('nan'), float('inf'), -1, 3601):
            for operation in (process.wait, process.reap, owner.collect):
                with self.subTest(timeout=timeout, operation=operation.__name__), \
                        self.assertRaisesRegex(ValueError, 'fixture_process_(wait_budget|deadline)_required'):
                    operation(timeout)

    def test_group_signals_finish_before_reap_and_repeat_cleanup_does_not_signal(self):
        with tempfile.TemporaryDirectory(prefix='apsp-native-owner-contract-') as directory:
            owner = object.__new__(fixture.OwnedProcess)
            process = SimpleNamespace(pid=9876, returncode=0, stdin=None, stdout=None, stderr=None, reaped=False)
            stages = []
            def observe(timeout):
                self.assertFalse(process.reaped)
                stages.append('observe')
                return 0
            def reap(timeout):
                self.assertFalse(process.reaped)
                stages.append('reap')
                process.reaped = True
                return 0
            def signal_group(pid, number):
                self.assertEqual(pid, 9876)
                self.assertFalse(process.reaped)
                stages.append(number)
            process.wait, process.reap = observe, reap
            owner.process, owner.command = process, ['TEST_ONLY never executed']
            owner.log = Path(directory) / 'owned.json'
            owner.selector = mock.Mock()
            owner.selector.get_map.return_value = {}
            owner.stdout, owner.stderr = bytearray(), bytearray()
            owner.failure, owner.cleanup_failures, owner.cleanup_confirmed = None, [], False
            with mock.patch.object(fixture, 'signal_group_if_live', side_effect=signal_group) as signals, \
                    mock.patch.object(fixture, 'group_has_live_members', return_value=False):
                owner.stop()
                owner.stop()
            self.assertEqual(stages, [signal.SIGTERM, 'observe', signal.SIGKILL, 'reap'])
            self.assertEqual(signals.call_count, 2)
            evidence = json.loads(owner.log.read_text())
            self.assertTrue(evidence['cleanup_confirmed'])
            self.assertEqual(evidence['cleanup_failures'], [])


if __name__ == '__main__':
    unittest.main()

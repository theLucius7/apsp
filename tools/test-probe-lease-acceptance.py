#!/usr/bin/env python3
"""Synthetic controller boundaries; never launch the native acceptance main."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import tempfile
import threading
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('probe_lease_controller', ROOT / 'tools/probe-lease-acceptance.py')
CONTROLLER = importlib.util.module_from_spec(spec)
spec.loader.exec_module(CONTROLLER)


class ControllerBoundaries(unittest.TestCase):
    def test_changed_or_untrusted_helper_manifest_rejects_before_any_exec(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            built = root / 'apsp-plugin-install-20261002-r1'
            built.mkdir()
            content = b'raise RuntimeError("helper must never execute in this fixture")\n'
            manifest = json.dumps({'script_sha256': {'common.py': hashlib.sha256(content).hexdigest(),
                                                    'finish.py': hashlib.sha256(content).hexdigest()}}).encode()
            (built / 'harness-identity.json').write_bytes(manifest)
            for helper in ('common.py', 'finish.py'):
                (built / helper).write_bytes(content)
            with mock.patch('builtins.exec', side_effect=AssertionError('native helper execution forbidden')) as execute:
                for trusted in (None, '0' * 64):
                    with self.subTest(trusted=trusted), self.assertRaisesRegex(CONTROLLER.ControlledFailure, 'trusted helper manifest'):
                        CONTROLLER.private_helpers(built, root / 'output', trusted)
                trusted = hashlib.sha256(manifest).hexdigest()
                for changed in ('common.py', 'finish.py'):
                    with self.subTest(changed=changed):
                        (built / changed).write_bytes(b'changed helper')
                        with self.assertRaisesRegex(CONTROLLER.ControlledFailure, 'helper digest'):
                            CONTROLLER.private_helpers(built, root / 'output', trusted)
                        (built / changed).write_bytes(content)
                execute.assert_not_called()

    def test_frozen_inputs_reject_symlink_fifo_hardlink_and_byte_overrun(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            ordinary = root / 'ordinary'
            ordinary.write_bytes(b'owned input')
            self.assertEqual(CONTROLLER.sha(ordinary), hashlib.sha256(b'owned input').hexdigest())
            with self.assertRaisesRegex(CONTROLLER.ControlledFailure, 'bounded ordinary'):
                CONTROLLER.read_private(ordinary, 3)
            linked = root / 'linked'
            linked.symlink_to(ordinary)
            with self.assertRaises(OSError):
                CONTROLLER.read_private(linked)
            fifo = root / 'fifo'
            os.mkfifo(fifo)
            with self.assertRaises(CONTROLLER.ControlledFailure):
                CONTROLLER.read_private(fifo)
            os.link(ordinary, root / 'hardlinked')
            with self.assertRaises(CONTROLLER.ControlledFailure):
                CONTROLLER.sha(ordinary)

    def test_finite_timer_and_cancel_handlers_restore_even_on_failure(self):
        previous = object()
        with mock.patch.object(signal, 'getitimer', return_value=(0.0, 0.0)), \
             mock.patch.object(signal, 'getsignal', return_value=previous), \
             mock.patch.object(signal, 'signal') as install, mock.patch.object(signal, 'setitimer') as timer:
            with self.assertRaisesRegex(ValueError, 'owned failure'):
                with CONTROLLER.finite_controller():
                    timer.assert_called_with(signal.ITIMER_REAL, CONTROLLER.CONTROLLER_SECONDS)
                    handlers = {call.args[0]: call.args[1] for call in install.call_args_list}
                    with self.assertRaises(CONTROLLER.ControllerDeadline):
                        handlers[signal.SIGALRM](signal.SIGALRM, None)
                    timer.assert_called_with(signal.ITIMER_REAL, 1)
                    with self.assertRaises(SystemExit) as cancelled:
                        handlers[signal.SIGTERM](signal.SIGTERM, None)
                    self.assertEqual(cancelled.exception.code, 128 + signal.SIGTERM)
                    raise ValueError('owned failure')
            timer.assert_called_with(signal.ITIMER_REAL, 0)
            restored = {call.args[0]: call.args[1] for call in install.call_args_list[-3:]}
            self.assertEqual(set(restored), {signal.SIGALRM, signal.SIGTERM, signal.SIGHUP})
            self.assertTrue(all(handler is previous for handler in restored.values()))
        with mock.patch.object(signal, 'getitimer', return_value=(1.0, 0.0)), \
             mock.patch.object(signal, 'signal') as install:
            with self.assertRaisesRegex(CONTROLLER.ControlledFailure, 'another active timer'):
                with CONTROLLER.finite_controller():
                    self.fail('another timer must remain owned')
            install.assert_not_called()

    def test_busy_private_state_backup_has_a_finite_callback_deadline(self):
        source, destination = mock.Mock(), mock.Mock()

        def busy_copy(_destination, **options):
            self.assertEqual(options['pages'], 128)
            self.assertEqual(options['sleep'], .05)
            options['progress'](5, 100, 100)

        source.backup.side_effect = busy_copy
        with mock.patch.object(CONTROLLER.sqlite3, 'connect', side_effect=[source, destination]) as connect, \
             mock.patch.object(CONTROLLER.time, 'monotonic', side_effect=[100, 111]):
            with self.assertRaisesRegex(CONTROLLER.ControlledFailure, 'backup deadline'):
                CONTROLLER.bounded_backup('owned-source', 'owned-destination')
        self.assertEqual(connect.call_args_list[0].kwargs, {'timeout': .5})
        source.close.assert_called_once()
        destination.close.assert_called_once()

    def test_gate_caps_concurrent_requests_without_binding_or_starting_threads(self):
        gate = object.__new__(CONTROLLER.Gate)
        gate.request_slots = threading.BoundedSemaphore(1)
        gate.shutdown_request = mock.Mock()
        request, address = object(), ('127.0.0.1', 1)
        with mock.patch.object(CONTROLLER.ThreadingHTTPServer, 'process_request') as dispatch, \
             mock.patch.object(CONTROLLER.ThreadingHTTPServer, 'process_request_thread') as handle:
            gate.process_request(request, address)
            dispatch.assert_called_once_with(request, address)
            gate.process_request(request, address)
            gate.shutdown_request.assert_called_once_with(request)
            gate.process_request_thread(request, address)
            handle.assert_called_once_with(request, address)
            self.assertTrue(gate.request_slots.acquire(blocking=False))

    def test_partial_request_body_rejects_before_any_panel_connection(self):
        handler = object.__new__(CONTROLLER.GateHandler)
        handler.server = mock.Mock(available=True)
        handler.path, handler.headers = '/owned', {'Content-Length': '3'}
        handler.rfile = mock.Mock()
        handler.rfile.read.return_value = b'x'
        with mock.patch.object(CONTROLLER.http.client, 'HTTPConnection') as connect:
            with self.assertRaisesRegex(CONTROLLER.ControlledFailure, 'complete bounded'):
                handler.exchange()
            connect.assert_not_called()


if __name__ == '__main__':
    unittest.main()

"""Bounded private inputs and process ownership for TEST_ONLY managed acceptance."""
import contextlib
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import stat
import subprocess
import sys
import time
import uuid

MAX_JSON = 8 * 1024 * 1024
MAX_OUTPUT = 256 * 1024


def require(condition, code):
    if not condition:
        raise ValueError(code)


def absolute(value):
    path = Path(value)
    require(path.is_absolute() and ".." not in path.parts, "absolute_normal_path_required")
    return path


def regular(path, maximum=MAX_JSON, allow_empty=False):
    path = absolute(path)
    require(not any(parent.is_symlink() for parent in path.parents), "input_parent_symlink_refused")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    fd = os.open(path, flags)
    try:
        before = os.fstat(fd)
        require(stat.S_ISREG(before.st_mode) and (before.st_size > 0 or allow_empty) and before.st_size <= maximum,
                "bounded_regular_input_required")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            data = stream.read(maximum + 1)
        after = os.fstat(fd)
        require(len(data) <= maximum and
                (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) ==
                (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns),
                "input_changed_or_exceeded_limit")
        return data
    finally:
        os.close(fd)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(path, maximum=256 * 1024 * 1024):
    data = regular(path, maximum, allow_empty=True)
    return {"sha256": digest(data), "size": len(data)}


def load(path):
    return json.loads(regular(path))


def write(path, value):
    data = (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode()
    require(len(data) <= MAX_JSON, "private_evidence_limit")
    fd = os.open(absolute(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as target:
        target.write(data)
        target.flush()
        os.fsync(target.fileno())


def owned_root(root, run_id):
    root = absolute(root)
    require(not root.is_symlink() and root.is_dir() and root.stat().st_uid == os.geteuid()
            and root.stat().st_mode & 0o077 == 0,
            "private_owned_run_root_required")
    require(str(uuid.UUID(run_id)) == run_id and uuid.UUID(run_id).int != 0, "canonical_run_uuid_required")
    require(regular(root / ".apsp-managed-test-run", 128).decode().strip() == run_id,
            "run_ownership_mismatch")
    return root


def within(root, value):
    path = absolute(value)
    require(path.is_relative_to(root) and path != root, "path_outside_owned_run")
    cursor = path
    while cursor != root:
        require(not cursor.is_symlink(), "owned_path_symlink_refused")
        cursor = cursor.parent
    return path


def group_has_live_members(pgid):
    # Native managed tools require Linux. Bound process-table observation too:
    # no unbounded ps capture and no signals based on a collected foreign row.
    require(Path("/proc/self/stat").is_file(), "linux_process_observation_required")
    end, count = time.monotonic() + 2, 0
    with os.scandir("/proc") as rows:
        for row in rows:
            if not row.name.isdigit():
                continue
            count += 1
            require(count <= 8192 and time.monotonic() < end, "cleanup_process_budget")
            try:
                with open(row.path + "/stat", "rb") as stream:
                    value = stream.read(8193)
            except (FileNotFoundError, ProcessLookupError):
                continue
            require(len(value) <= 8192 and b") " in value, "cleanup_observation_failed")
            fields = value.rsplit(b") ", 1)[1].split()
            require(len(fields) >= 3, "cleanup_observation_failed")
            if int(fields[2]) == pgid and fields[0] != b"Z":
                return True
    return False


def signal_group_if_live(pgid, number):
    if not group_has_live_members(pgid):
        return
    try:
        os.killpg(pgid, number)
    except ProcessLookupError:
        pass
    except PermissionError:
        # Darwin can return EPERM after TERM has left only orphan zombies.
        require(not group_has_live_members(pgid), "owned_group_cleanup_permission_denied")


def write_pipe(stream, data, seconds=5):
    """Finite, nonblocking stdin; a stuck peer cannot bypass its deadline."""
    require(isinstance(data, bytes) and len(data) <= MAX_JSON and 0 < seconds <= 30,
            "owned_process_input_budget")
    os.set_blocking(stream.fileno(), False)
    end = time.monotonic() + seconds
    with selectors.DefaultSelector() as selector:
        selector.register(stream, selectors.EVENT_WRITE)
        pending = memoryview(data)
        while pending:
            remaining = end - time.monotonic()
            require(remaining > 0, "owned_process_input_timeout")
            for key, _ in selector.select(min(0.1, remaining)):
                try:
                    count = os.write(key.fd, pending[:4096])
                except BlockingIOError:
                    continue
                require(count > 0, "owned_process_input_closed")
                pending = pending[count:]


@contextlib.contextmanager
def cleanup_signals():
    previous = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT, signal.SIGTERM, signal.SIGHUP})
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)


def require_linux_child_observation():
    require(hasattr(os, "waitid") and hasattr(os, "WNOWAIT"),
            "nonreaping_child_observation_required")
    require(sys.platform == "linux" and Path("/proc/self/stat").is_file(),
            "linux_process_observation_required")


class OwnedProcess(subprocess.Popen):
    """Observe without reaping; release a group leader only after cleanup."""

    def __init__(self, *args, **kwargs):
        require_linux_child_observation()
        require(kwargs.get("start_new_session") is True, "owned_process_session_required")
        self._owned_reaped = False
        super().__init__(*args, **kwargs)

    def poll(self):
        if self._owned_reaped:
            return self.returncode
        observed = os.waitid(os.P_PID, self.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if observed is not None:
            self.returncode = (observed.si_status if observed.si_code == os.CLD_EXITED
                               else -observed.si_status)
        return self.returncode

    def wait(self, timeout=None):
        require(timeout is not None and timeout >= 0, "owned_process_wait_budget_required")
        end = time.monotonic() + timeout
        while self.poll() is None:
            if time.monotonic() >= end:
                raise subprocess.TimeoutExpired(self.args, timeout)
            time.sleep(min(0.02, max(0, end - time.monotonic())))
        return self.returncode

    def reap(self, timeout=3):
        if not self._owned_reaped:
            # Popen otherwise treats an observed return code as already reaped.
            self.returncode = None
            result = super().wait(timeout=timeout)
            self._owned_reaped = True
            return result
        return self.returncode

    def stop_group(self):
        with cleanup_signals():
            OwnedProcess._stop_group(self)

    def _stop_group(self):
        if self._owned_reaped:
            return
        try:
            try:
                os.killpg(self.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.wait(timeout=2)
            except subprocess.TimeoutExpired:
                pass
        finally:
            # All signals precede reap; the unreaped leader pins the group ID.
            try:
                os.killpg(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.reap(timeout=3)
        end = time.monotonic() + 2
        while group_has_live_members(self.pid):
            require(time.monotonic() < end, "owned_group_cleanup_not_confirmed")
            time.sleep(0.02)


def capture(command, timeout=30, input_bytes=None, cwd=None, env=None, log=None, guard=None, pass_fds=()):
    """Kill our complete group even when a parent exits with a pipe-holding child."""
    require(timeout > 0 and (input_bytes is None or len(input_bytes) <= MAX_OUTPUT),
            "command_budget_invalid")
    require_linux_child_observation()
    process = subprocess.Popen(command, stdin=subprocess.PIPE if input_bytes is not None else subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
                               cwd=cwd, env=env, pass_fds=pass_fds)
    deadline = time.monotonic() + timeout
    output, errors, failure = bytearray(), bytearray(), None
    selector = selectors.DefaultSelector()
    for stream, target in ((process.stdout, output), (process.stderr, errors)):
        os.set_blocking(stream.fileno(), False)
        selector.register(stream, selectors.EVENT_READ, target)
    if input_bytes is not None:
        os.set_blocking(process.stdin.fileno(), False)
        selector.register(process.stdin, selectors.EVENT_WRITE, memoryview(input_bytes))
    completed = False
    try:
        while selector.get_map() or not completed:
            if not completed:
                observed = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                if observed is not None:
                    completed = True
                    returncode = (observed.si_status if observed.si_code == os.CLD_EXITED
                                  else -observed.si_status)
            require(time.monotonic() < deadline, "controlled_command_timeout")
            if guard is not None:
                guard()
            for key, _ in selector.select(min(0.25, max(0, deadline - time.monotonic()))):
                if key.fileobj is process.stdin:
                    try:
                        consumed = os.write(key.fd, key.data[:4096])
                    except BrokenPipeError:
                        consumed = len(key.data)
                    pending = key.data[consumed:]
                    if pending:
                        selector.modify(key.fileobj, selectors.EVENT_WRITE, pending)
                    else:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                    continue
                block = os.read(key.fd, 4096)
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                else:
                    allowance = MAX_OUTPUT - len(output) - len(errors)
                    key.data.extend(block[:allowance])
                    require(len(block) <= allowance, "controlled_command_output_limit")
        require(returncode == 0, "controlled_command_failed")
        return bytes(output)
    except BaseException as error:
        failure = type(error).__name__
        raise
    finally:
        with cleanup_signals():
            cleanup_failure = None
            try:
                signal_group_if_live(process.pid, signal.SIGTERM)
                term_deadline = time.monotonic() + 2
                while group_has_live_members(process.pid) and time.monotonic() < term_deadline:
                    time.sleep(0.02)
                # WNOWAIT retains the leader even if its command has exited.
                signal_group_if_live(process.pid, signal.SIGKILL)
                process.wait(timeout=3)
                cleanup_deadline = time.monotonic() + 2
                while group_has_live_members(process.pid):
                    require(time.monotonic() < cleanup_deadline, "owned_group_cleanup_not_confirmed")
                    time.sleep(0.02)
            except BaseException as error:
                cleanup_failure = type(error).__name__
                raise
            finally:
                for key in list(selector.get_map().values()):
                    key.fileobj.close()
                selector.close()
                if log is not None:
                    write(log, {"returncode": process.returncode, "failure_type": failure,
                                "cleanup_failure_type": cleanup_failure,
                                "stdout": output.decode("utf-8", "replace"),
                                "stderr": errors.decode("utf-8", "replace")})

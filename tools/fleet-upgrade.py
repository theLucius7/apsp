#!/usr/bin/env python3
"""Plan or sequentially upgrade an explicit fleet through the existing panel API.

Only --apply changes Agent settings or requests fresh runtime checkpoints. Target
runtime identity is derived strictly from the signed self-contained Agent release
and verified embedded component reports; operators never maintain or choose a
separate runtime version. External client checks remain separate from panel health.
"""

import argparse
import contextlib
import fcntl
import http.cookies
import ipaddress
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

SEGMENT_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}\Z")
SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")


class UpgradeError(Exception):
    """A bounded, credential-free operational failure."""


def stable_version(value):
    if not isinstance(value, str) or not re.fullmatch(
        r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", value
    ):
        raise UpgradeError("agent_version_must_be_stable_semver")
    return tuple(map(int, value.split(".")))


def origin(value):
    parsed = urllib.parse.urlsplit(value)
    if (parsed.username or parsed.password or parsed.query
            or parsed.fragment or parsed.path not in ("", "/")):
        raise UpgradeError("panel_origin_must_not_contain_credentials_or_a_path")
    try:
        local = ipaddress.ip_address(parsed.hostname or "").is_loopback
    except ValueError:
        local = False
    if not parsed.hostname or (parsed.scheme != "https" and not (parsed.scheme == "http" and local)):
        raise UpgradeError("panel_requires_https_or_explicit_loopback")
    return value.rstrip("/")


def private_json(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        metadata = os.fstat(source.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid()
                or metadata.st_mode & 0o077 or metadata.st_size > 1024 * 1024):
            raise UpgradeError("private_file_must_be_owned_regular_and_mode_0600")
        try:
            return json.load(source)
        except (ValueError, UnicodeError):
            raise UpgradeError("invalid_private_json") from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return None


class Panel:
    def __init__(self, base, public, credentials):
        self.base, self.public = origin(base), origin(public)
        self.credentials = credentials
        self.cookie = None
        self.opener = urllib.request.build_opener(NoRedirect())

    def call(self, path, method="GET", payload=None, timeout=45):
        if not path.startswith("/api/") or "\r" in path or "\n" in path:
            raise UpgradeError("invalid_api_path")
        headers = {"Origin": self.public, "User-Agent": "apsp-fleet-upgrade/1"}
        if self.cookie:
            headers["Cookie"] = self.cookie
        body = None
        if payload is not None:
            headers["Content-Type"] = "application/json"
            body = json.dumps(payload).encode()
        request = urllib.request.Request(self.base + path, data=body, method=method, headers=headers)
        try:
            with self.opener.open(request, timeout=timeout) as response:
                for value in response.headers.get_all("Set-Cookie", []):
                    cookies = http.cookies.SimpleCookie(value)
                    if "apsp_session" in cookies:
                        self.cookie = cookies["apsp_session"].OutputString(attrs=[])
                raw = response.read(8 * 1024 * 1024 + 1)
                if len(raw) > 8 * 1024 * 1024:
                    raise UpgradeError("panel_response_exceeds_limit")
                return json.loads(raw) if raw.strip() else None
        except urllib.error.HTTPError as error:
            raise UpgradeError(f"panel_http_{error.code}") from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise UpgradeError("panel_network_error") from None
        except (ValueError, UnicodeError, http.cookies.CookieError):
            raise UpgradeError("panel_invalid_json_or_cookie") from None

    def login(self):
        self.call("/api/login", "POST", self.credentials)
        if not self.cookie:
            raise UpgradeError("panel_session_missing")

    def logout(self):
        if self.cookie:
            self.call("/api/logout", "POST", {})
            self.cookie = None


class Receipt:
    """A private recovery record, written before enabling an Agent's update setting."""

    def __init__(self, path, ids, version):
        self.path = Path(path)
        self.value = {
            "schema": 1,
            "servers": ids,
            "agent_version": version,
            "members": {},
        }
        self.lock = None

    def __enter__(self):
        if not self.path.parent.is_dir():
            raise UpgradeError("receipt_parent_must_exist")
        descriptor = os.open(
            str(self.path) + ".lock", os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600
        )
        self.lock = os.fdopen(descriptor, "w")
        metadata = os.fstat(descriptor)
        if (not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.getuid()
                or metadata.st_mode & 0o077):
            self.lock.close()
            raise UpgradeError("receipt_lock_must_be_private")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            self.lock.close()
            raise UpgradeError("receipt_is_in_use") from None
        if self.path.exists() or self.path.is_symlink():
            old = private_json(self.path)
            if any(old.get(k) != self.value[k] for k in ("schema", "servers", "agent_version")):
                raise UpgradeError("receipt_does_not_match_requested_plan")
            self.value = old
        return self

    def save(self):
        if self.path.is_symlink():
            raise UpgradeError("receipt_must_not_be_a_symlink")
        descriptor, temporary = tempfile.mkstemp(
            prefix="." + self.path.name + ".", dir=self.path.parent
        )
        try:
            with os.fdopen(descriptor, "w") as output:
                json.dump(self.value, output, sort_keys=True)
                output.write("\n")
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, self.path)
        finally:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(temporary)

    def __exit__(self, *unused):
        if self.lock:
            self.lock.close()


def agent_targets(info):
    """Match governance::releases, including its static-Agent legacy key."""
    arch = {"x86_64": "amd64", "amd64": "amd64", "aarch64": "arm64", "arm64": "arm64"}.get(
        info.get("arch")
    )
    system, libc = info.get("os"), info.get("libc")
    if not arch:
        raise UpgradeError("unknown_agent_architecture")
    if system == "linux" and libc in ("musl", "gnu", "glibc"):
        target = f"linux-{'gnu' if libc == 'glibc' else libc}-{arch}"
        return [target, arch] if libc == "musl" else [target]
    if (system, arch) in (
        ("macos", "arm64"),
        ("freebsd", "amd64"),
        ("freebsd", "arm64"),
        ("windows", "amd64"),
        ("windows", "arm64"),
    ):
        return [f"{system}-{arch}"]
    raise UpgradeError("unknown_agent_platform")


def candidate(panel, server_value, expected):
    targets = agent_targets(server_value["static_info"])
    versions = panel.call("/api/system/agent-releases")["versions"]
    choices = []
    for item in versions:
        try:
            key = stable_version(item.get("version"))
        except UpgradeError:
            continue
        pmin, pmax = item.get("protocol_min", 1), item.get("protocol_max", 1)
        if not (type(pmin) is int and type(pmax) is int and pmin <= 1 <= pmax):
            continue
        available = item.get("targets", [])
        matched = next((t for t in targets if t in available), None)
        if matched is not None:
            choices.append((key, item, matched))
    if not choices:
        raise UpgradeError("signed_agent_candidate_missing")
    _, selected, matched_target = max(choices, key=lambda entry: entry[0])
    if selected.get("version") != expected or selected.get("tag") != "agent-v" + expected:
        raise UpgradeError("signed_agent_candidate_changed")
    return selected, matched_target


def catalog_embedded_runtime(selected, matched_target):
    """Extract target sing-box version/digest and Agent digest when exposed by the panel catalog."""
    rt_ver, rt_sha, agent_sha = None, None, None
    digests = selected.get("target_digests")
    if isinstance(digests, dict) and matched_target in digests:
        raw_agent_sha = digests[matched_target]
        if not isinstance(raw_agent_sha, str) or not SHA256_RE.fullmatch(raw_agent_sha):
            raise UpgradeError("invalid_catalog_target_digest")
        agent_sha = raw_agent_sha
    for field in ("target_components", "embedded_components", "components"):
        container = selected.get(field)
        if not isinstance(container, dict):
            continue
        comp_map = container.get(matched_target) if isinstance(container.get(matched_target), dict) else container
        if not isinstance(comp_map, dict) or "sing-box" not in comp_map:
            continue
        entry = comp_map["sing-box"]
        if isinstance(entry, str):
            if not SEGMENT_RE.fullmatch(entry):
                raise UpgradeError("invalid_catalog_embedded_runtime")
            rt_ver = entry
        elif isinstance(entry, dict):
            ver = entry.get("version")
            sha = entry.get("sha256")
            if not isinstance(ver, str) or not SEGMENT_RE.fullmatch(ver):
                raise UpgradeError("invalid_catalog_embedded_runtime")
            if sha is not None and (not isinstance(sha, str) or not SHA256_RE.fullmatch(sha)):
                raise UpgradeError("invalid_catalog_embedded_runtime")
            rt_ver, rt_sha = ver, sha
        else:
            raise UpgradeError("invalid_catalog_embedded_runtime")
        break
    return rt_ver, rt_sha, agent_sha


def verified_target_runtime(current, runtime_meta, selected, matched_target):
    """Derive and cross-check the running embedded sing-box version after target Agent verification."""
    static_info = current["static_info"]
    if static_info.get("agent_version") != selected.get("version"):
        return None
    if runtime_meta.get("agent_supported") is False:
        return None
    caps = current.get("capabilities")
    if isinstance(caps, list) and "self_contained_agent" not in caps:
        return None

    catalog_rt, catalog_rt_sha, catalog_agent_sha = catalog_embedded_runtime(selected, matched_target)
    comp_digests = static_info.get("component_digests")
    if comp_digests is not None and not isinstance(comp_digests, dict):
        raise UpgradeError("target_component_digests_invalid")
    primary_agent_sha = comp_digests.get("agent") if isinstance(comp_digests, dict) else None
    alias_agent_sha = static_info.get("agent_sha256")
    for sha in (primary_agent_sha, alias_agent_sha):
        if sha is not None and (not isinstance(sha, str) or not SHA256_RE.fullmatch(sha)):
            raise UpgradeError("target_agent_identity_digest_invalid")
    if primary_agent_sha and alias_agent_sha and primary_agent_sha != alias_agent_sha:
        raise UpgradeError("target_agent_identity_digest_conflict")
    observed_agent_sha = primary_agent_sha or alias_agent_sha
    if catalog_agent_sha is not None:
        if not observed_agent_sha:
            return None
        if observed_agent_sha != catalog_agent_sha:
            raise UpgradeError("target_agent_digest_mismatch")

    components = static_info.get("components")
    if not isinstance(components, dict):
        return None
    embedded_rt = components.get("sing-box")
    if not isinstance(embedded_rt, str) or not SEGMENT_RE.fullmatch(embedded_rt):
        return None
    if catalog_rt is not None and embedded_rt != catalog_rt:
        raise UpgradeError("embedded_runtime_differs_from_signed_release")

    if isinstance(comp_digests, dict) and "sing-box" in comp_digests:
        obs_rt_sha = comp_digests["sing-box"]
        if not isinstance(obs_rt_sha, str) or not SHA256_RE.fullmatch(obs_rt_sha):
            raise UpgradeError("embedded_runtime_digest_invalid")
        if catalog_rt_sha is not None and obs_rt_sha != catalog_rt_sha:
            raise UpgradeError("embedded_runtime_digest_mismatch")
    elif catalog_rt_sha is not None:
        return None

    reported_rt = static_info.get("runtime_version")
    if not isinstance(reported_rt, str) or reported_rt != embedded_rt:
        return None
    panel_rt = runtime_meta.get("runtime_version")
    if panel_rt is not None and panel_rt != embedded_rt:
        return None
    return embedded_rt


def server(panel, identifier):
    value = panel.call(f"/api/servers/{identifier}")
    if value.get("id") != identifier or not isinstance(value.get("static_info"), dict):
        raise UpgradeError("server_identity_missing_or_changed")
    if type(value.get("agent_settings", {}).get("auto_update")) is not bool:
        raise UpgradeError("auto_update_setting_missing")
    return value


def runtime(panel, identifier):
    value = panel.call(f"/api/traffic/servers/{identifier}")
    if (value.get("id") != identifier
            or type(value.get("configured")) is not bool
            or not isinstance(value.get("status"), dict)):
        raise UpgradeError("runtime_state_missing")
    return value


def set_auto(panel, identifier, enabled):
    current = server(panel, identifier)
    panel.call(f"/api/servers/{identifier}", "PATCH", {"name": current["name"], "auto_update": enabled})
    if server(panel, identifier)["agent_settings"]["auto_update"] is not enabled:
        raise UpgradeError("auto_update_change_not_confirmed")


def valid_binding(binding):
    return (
        isinstance(binding, dict)
        and isinstance(binding.get("deployment_id"), str)
        and bool(binding["deployment_id"])
        and binding.get("module") == "singbox"
        and type(binding.get("revision")) is int
        and binding["revision"] > 0
        and isinstance(binding.get("bundle_sha256"), str)
        and bool(SHA256_RE.fullmatch(binding["bundle_sha256"]))
        and isinstance(binding.get("binding_digest"), str)
        and bool(SHA256_RE.fullmatch(binding["binding_digest"]))
    )


def runtime_preconditions_met(ready_reply, current_agent_version, target_agent_version, metadata):
    if ready_reply.get("ready") is True:
        return True
    checks = ready_reply.get("checks")
    if (isinstance(checks, list) and checks
            and metadata.get("status", {}).get("state") == "upgrade_required"
            and stable_version(current_agent_version) < stable_version(target_agent_version)):
        return all(
            isinstance(item, dict)
            and (item.get("passed") is True or item.get("name") in ("内置运行时能力", "运行时能力"))
            for item in checks
        )
    return False


def runtime_ready(panel, current, metadata, expected, allow_unconfirmed_health=False, expected_revision=None):
    static_info = current["static_info"]
    if static_info.get("runtime_version") != expected:
        return False
    components = static_info.get("components")
    if not isinstance(components, dict) or components.get("sing-box") != expected:
        return False
    allowed_states = ("ready", "pending") if allow_unconfirmed_health else ("ready",)
    if metadata.get("status", {}).get("state") not in allowed_states:
        return False
    deployment = panel.call(f"/api/traffic/servers/{current['id']}/deployments")
    state = deployment.get("status") or {}
    healthy = state.get("healthy") is True or allow_unconfirmed_health
    revision_ok = expected_revision is None or state.get("target_rev") == expected_revision
    return bool(
        not deployment.get("pending")
        and healthy
        and revision_ok
        and state.get("target_rev", 0) > 0
        and state.get("target_rev") == state.get("applied_rev")
        and not state.get("last_error")
    )


def request_checkpoint(panel, identifier):
    requested = panel.call(
        f"/api/servers/{identifier}/runtime-control/checkpoint", "POST", {"module": "singbox"}
    )
    request = requested.get("request") if isinstance(requested, dict) else None
    if (not isinstance(request, dict)
            or not isinstance(request.get("request_id"), str)
            or not request["request_id"]
            or not valid_binding(request.get("expected"))
            or type(request.get("expires_at")) is not int):
        raise UpgradeError("runtime_checkpoint_request_invalid")
    return request


def checkpoint_state(panel, identifier, request):
    request_id, expected = request["request_id"], request["expected"]
    value = panel.call(f"/api/servers/{identifier}/runtime-control")
    if not isinstance(value, dict) or not isinstance(value.get("requests"), list):
        raise UpgradeError("runtime_control_state_missing")
    matched = next(
        (item for item in value["requests"] if isinstance(item, dict) and item.get("request_id") == request_id),
        None,
    )
    if matched is None:
        return "retry"
    if matched.get("kind") not in (None, "checkpoint") or matched.get("module") not in (None, "singbox"):
        raise UpgradeError("runtime_checkpoint_rejected")
    if matched.get("outcome") in ("rejected", "failed", "mismatch"):
        raise UpgradeError("runtime_checkpoint_rejected")
    if matched.get("state") in ("expired", "superseded") or matched.get("outcome") in ("late", "superseded"):
        return "retry"
    created_at = matched.get("created_at")
    if type(created_at) is not int:
        raise UpgradeError("runtime_checkpoint_timestamp_missing")
    if matched.get("outcome") == "verified":
        received_at = matched.get("received_at")
        if type(received_at) is not int or received_at < created_at:
            return "pending"
        result = matched.get("result")
        observed = result.get("observed") if isinstance(result, dict) else None
        if (not isinstance(result, dict)
                or result.get("success") is not True
                or result.get("request_id") not in (None, request_id)
                or (matched.get("request_digest") is not None
                    and result.get("request_digest") not in (None, matched.get("request_digest")))
                or not isinstance(observed, dict)
                or observed.get("healthy") is not True
                or observed.get("binding") != expected):
            raise UpgradeError("runtime_checkpoint_rejected")
        checkpoints = value.get("checkpoints")
        if checkpoints is not None:
            if not isinstance(checkpoints, list):
                raise UpgradeError("runtime_control_state_missing")
            module_checkpoint = next(
                (entry for entry in checkpoints if isinstance(entry, dict) and entry.get("module") == "singbox"),
                None,
            )
            if module_checkpoint is None or module_checkpoint.get("current") is not True:
                return "retry"
            confirmed = module_checkpoint.get("checkpoint")
            verified_at = module_checkpoint.get("verified_at")
            if (not isinstance(confirmed, dict)
                    or confirmed.get("healthy") is not True
                    or confirmed.get("binding") != expected):
                return "retry"
            if type(verified_at) is not int or verified_at < created_at:
                return "pending"
        return "verified"
    return "pending"


def upgrade_one(
    panel,
    identifier,
    version,
    receipt,
    timeout,
    interval,
    client_check=None,
    clock=time.monotonic,
    sleep=time.sleep,
):
    started = clock()
    current = server(panel, identifier)
    if not current.get("online"):
        raise UpgradeError("server_offline")
    selected, matched_target = candidate(panel, current, version)
    if stable_version(current["static_info"].get("agent_version")) > stable_version(version):
        raise UpgradeError("agent_downgrade_not_supported")
    metadata = runtime(panel, identifier)
    has_runtime = metadata["configured"]
    if has_runtime and not client_check:
        raise UpgradeError("external_client_check_required_before_runtime_upgrade")
    locked_runtime, _, _ = catalog_embedded_runtime(selected, matched_target) if has_runtime else (None, None, None)
    previous = current["agent_settings"]["auto_update"]
    member = {
        "mode": "runtime" if has_runtime else "monitor",
        "original_auto_update": previous,
        "restore_required": True,
        "state": "upgrading",
        "external_client_check": "not_run" if has_runtime else "not_applicable",
    }
    receipt.value["members"][str(identifier)] = member
    receipt.save()
    try:
        if current["static_info"].get("agent_version") != version:
            set_auto(panel, identifier, True)
        checkpoint = None
        retry_checkpoint = False
        while clock() - started < timeout:
            current = server(panel, identifier)
            selected, matched_target = candidate(panel, current, version)
            observed_version = current["static_info"].get("agent_version")
            if stable_version(observed_version) > stable_version(version):
                raise UpgradeError("unexpected_agent_version")
            latest_runtime = runtime(panel, identifier)
            if latest_runtime["configured"] != has_runtime:
                raise UpgradeError("server_runtime_mode_changed")
            if current.get("online") and observed_version == version:
                if current["agent_settings"]["auto_update"] != previous:
                    set_auto(panel, identifier, previous)
                if not has_runtime:
                    member["state"] = "control_plane_verified"
                    return member
                verified_rt = verified_target_runtime(current, latest_runtime, selected, matched_target)
                if verified_rt is not None:
                    if locked_runtime is None:
                        locked_runtime = verified_rt
                    elif verified_rt != locked_runtime:
                        raise UpgradeError("embedded_runtime_identity_changed")
                if locked_runtime is not None and (
                    checkpoint is not None
                    or runtime_ready(
                        panel,
                        current,
                        latest_runtime,
                        locked_runtime,
                        allow_unconfirmed_health=retry_checkpoint,
                    )
                ):
                    if checkpoint is None:
                        checkpoint = request_checkpoint(panel, identifier)
                        retry_checkpoint = False
                    status = checkpoint_state(panel, identifier, checkpoint)
                    if status == "retry":
                        checkpoint = None
                        retry_checkpoint = True
                    elif status == "verified":
                        current = server(panel, identifier)
                        selected, matched_target = candidate(panel, current, version)
                        confirmed_runtime = runtime(panel, identifier)
                        if confirmed_runtime["configured"] != has_runtime:
                            raise UpgradeError("server_runtime_mode_changed")
                        reverified = verified_target_runtime(
                            current, confirmed_runtime, selected, matched_target
                        )
                        if reverified != locked_runtime or not runtime_ready(
                            panel,
                            current,
                            confirmed_runtime,
                            locked_runtime,
                            expected_revision=checkpoint["expected"]["revision"],
                        ):
                            checkpoint = None
                            retry_checkpoint = True
                        else:
                            member["target_runtime_version"] = locked_runtime
                            if client_check:
                                try:
                                    result = subprocess.run(
                                        [str(client_check), str(identifier), locked_runtime],
                                        stdin=subprocess.DEVNULL,
                                        stdout=subprocess.DEVNULL,
                                        stderr=subprocess.DEVNULL,
                                        timeout=max(1, timeout - (clock() - started)),
                                        check=False,
                                    )
                                except (OSError, subprocess.TimeoutExpired):
                                    raise UpgradeError("external_client_check_failed") from None
                                if result.returncode:
                                    raise UpgradeError("external_client_check_failed")
                                member["external_client_check"] = "passed"
                            member["state"] = "control_plane_verified"
                            return member
            sleep(interval)
        raise UpgradeError("upgrade_or_runtime_verification_timed_out")
    except BaseException:
        member["state"] = "failed"
        raise
    finally:
        try:
            set_auto(panel, identifier, previous)
            member["restore_required"] = False
        except BaseException:
            member["state"] = "restore_required"
            receipt.save()
            raise UpgradeError("original_auto_update_restore_failed") from None
        receipt.save()


def recover_settings(panel, receipt):
    for key, member in receipt.value["members"].items():
        if member.get("restore_required"):
            set_auto(panel, int(key), member["original_auto_update"])
            member["restore_required"] = False
            member["state"] = "recovered_settings"
            receipt.save()


def emit(value):
    print(json.dumps(value, ensure_ascii=False, sort_keys=True), flush=True)


def plan(panel, ids, version):
    result = []
    for identifier in ids:
        item = {"server_id": identifier, "ready": False}
        try:
            current = server(panel, identifier)
            selected, matched_target = candidate(panel, current, version)
            metadata = runtime(panel, identifier)
            current_agent_version = current["static_info"].get("agent_version")
            item.update(
                mode="runtime" if metadata["configured"] else "monitor",
                online=bool(current.get("online")),
                current_agent_version=current_agent_version,
                target_agent_version=version,
                target_agent_target=matched_target,
                original_auto_update=current["agent_settings"]["auto_update"],
            )
            if not current.get("online"):
                raise UpgradeError("server_offline")
            if stable_version(current_agent_version) > stable_version(version):
                raise UpgradeError("agent_downgrade_not_supported")
            if metadata["configured"]:
                ready = panel.call(f"/api/traffic/servers/{identifier}/deployments/check", "POST", {})
                if not runtime_preconditions_met(ready, current_agent_version, version, metadata):
                    raise UpgradeError("runtime_preconditions_or_signed_artifact_missing")
                catalog_rt, _, _ = catalog_embedded_runtime(selected, matched_target)
                if current_agent_version == version:
                    derived_rt = verified_target_runtime(current, metadata, selected, matched_target)
                    if derived_rt is None:
                        raise UpgradeError("target_embedded_runtime_evidence_missing")
                    source = "signed_release_manifest" if catalog_rt else "verified_target_agent"
                else:
                    derived_rt = catalog_rt
                    source = "signed_release_manifest" if catalog_rt else "target_signed_agent_report"
                item.update(
                    current_runtime_version=current["static_info"].get("runtime_version"),
                    target_runtime_version=derived_rt,
                    target_runtime_source=source,
                    external_client_check="not_run",
                )
            item["ready"] = True
        except UpgradeError as error:
            item["error"] = str(error)
        result.append(item)
    return result


def prepare_apply(panel, ids, version, targets, client_check):
    for identifier in ids:
        current = server(panel, identifier)
        if not current.get("online"):
            raise UpgradeError("server_offline")
        if runtime(panel, identifier)["configured"] and not client_check:
            raise UpgradeError("external_client_check_required_before_runtime_upgrade")
    if targets:
        supported = panel.call("/api/system/agent-releases/targets")["supported_targets"]
        if len(set(targets)) != len(targets) or any(target not in supported for target in targets):
            raise UpgradeError("invalid_import_targets")
        result = panel.call(
            "/api/system/agent-releases/import",
            "POST",
            {"tag": "agent-v" + version, "targets": targets},
            timeout=900,
        )
        if result.get("signature_verified") is not True:
            raise UpgradeError("signed_release_import_not_confirmed")


def execute(panel, ids, version, receipt, timeout, interval, client_check=None):
    recover_settings(panel, receipt)
    members = []
    for position, identifier in enumerate(ids):
        try:
            member = upgrade_one(
                panel, identifier, version, receipt, timeout, interval, client_check
            )
            members.append({"server_id": identifier, **member})
            emit({"event": "server_completed", "server_id": identifier, **member})
        except (UpgradeError, KeyboardInterrupt) as error:
            reason = "interrupted" if isinstance(error, KeyboardInterrupt) else str(error)
            members.append({"server_id": identifier, "state": "failed", "error": reason})
            members.extend(
                {"server_id": remaining, "state": "not_started"}
                for remaining in ids[position + 1:]
            )
            break
    verified = len(members) == len(ids) and all(
        item["state"] == "control_plane_verified" for item in members
    )
    clients = verified and all(
        item.get("external_client_check") in ("passed", "not_applicable") for item in members
    )
    result = {
        "event": "fleet_summary",
        "control_plane_complete": verified,
        "complete": clients,
        "external_client_acceptance": "passed_or_not_applicable" if clients else "not_complete",
        "members": members,
    }
    emit(result)
    return 0 if clients else 3 if verified else 1


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--servers", nargs="+", required=True, type=int, help="ordered explicit server IDs")
    parser.add_argument("--agent-version", required=True)
    parser.add_argument("--api-url", default=os.environ.get("APSP_API_URL"))
    parser.add_argument("--public-origin", default=os.environ.get("APSP_PUBLIC_ORIGIN"))
    parser.add_argument(
        "--credentials",
        type=Path,
        help="private JSON with password and optional login_name/totp_code",
    )
    parser.add_argument("--apply", action="store_true")
    parser.add_argument(
        "--import-targets",
        nargs="+",
        default=[],
        help="explicit signed-release platforms to import before applying",
    )
    parser.add_argument("--receipt", type=Path, help="private recovery record; required with --apply")
    parser.add_argument("--timeout", type=int, default=900, help="deadline per server in seconds")
    parser.add_argument("--interval", type=int, default=10)
    parser.add_argument(
        "--client-check",
        type=Path,
        help="executable receiving server ID and derived embedded runtime version; no shell",
    )
    return parser.parse_args(argv)


def main():
    args = arguments()
    stable_version(args.agent_version)
    if not args.api_url or len(set(args.servers)) != len(args.servers) or any(i <= 0 for i in args.servers):
        raise UpgradeError("api_url_and_unique_positive_server_ids_required")
    if args.timeout <= 0 or not 1 <= args.interval <= 60 or (args.apply and not args.receipt):
        raise UpgradeError("valid_deadline_interval_and_apply_receipt_required")
    credentials = (
        private_json(args.credentials)
        if args.credentials
        else {"password": os.environ.get("APSP_PASSWORD", "")}
    )
    if (not isinstance(credentials, dict)
            or not credentials.get("password")
            or set(credentials) - {"password", "login_name", "totp_code"}):
        raise UpgradeError("password_credentials_required")
    panel = Panel(args.api_url, args.public_origin or args.api_url, credentials)

    def interrupted(*unused):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    panel.login()
    try:
        recovery = (
            Receipt(args.receipt, args.servers, args.agent_version)
            if args.apply
            else contextlib.nullcontext(None)
        )
        with recovery as receipt:
            if args.apply:
                recover_settings(panel, receipt)
                prepare_apply(panel, args.servers, args.agent_version, args.import_targets, args.client_check)
            planned = plan(panel, args.servers, args.agent_version)
            emit({
                "event": "fleet_plan",
                "apply": args.apply,
                "members": planned,
                "requested_import_targets": args.import_targets,
                "candidate_policy": "highest_signed_stable_must_equal_fixed_version_no_concurrent_release_import",
            })
            if not args.apply:
                return 0 if all(item["ready"] for item in planned) else 1
            if not all(item["ready"] for item in planned):
                emit({
                    "event": "fleet_summary",
                    "complete": False,
                    "control_plane_complete": False,
                    "error": "plan_not_ready",
                })
                return 1
            return execute(
                panel, args.servers, args.agent_version, receipt, args.timeout, args.interval, args.client_check
            )
    finally:
        with contextlib.suppress(UpgradeError):
            panel.logout()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except UpgradeError as error:
        emit({"event": "fleet_error", "error": str(error)})
        raise SystemExit(2) from None

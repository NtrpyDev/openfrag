#!/usr/bin/env python3
"""Exercise the shipped daemon through local process and loopback HTTP seams."""

from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import signal
import socket
import stat
import subprocess
import tempfile
import time
from typing import Any
import urllib.error
import urllib.request


class SmokeFailure(RuntimeError):
    """A failed observable smoke contract."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SmokeFailure(message)


def run(command: list[str], environment: dict[str, str]) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        env=environment,
        text=True,
        capture_output=True,
        timeout=15,
        check=False,
    )
    if result.returncode != 0:
        raise SmokeFailure(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def reserve_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def request(
    opener: urllib.request.OpenerDirector,
    url: str,
    *,
    method: str = "GET",
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, str], bytes]:
    http_request = urllib.request.Request(
        url,
        data=body,
        headers=headers or {},
        method=method,
    )
    try:
        with opener.open(http_request, timeout=2) as response:
            return (
                response.status,
                {key.lower(): value for key, value in response.headers.items()},
                response.read(),
            )
    except urllib.error.HTTPError as error:
        return (
            error.code,
            {key.lower(): value for key, value in error.headers.items()},
            error.read(),
        )


def wait_for_health(
    opener: urllib.request.OpenerDirector,
    base_url: str,
    daemon: subprocess.Popen[bytes],
    daemon_log: Path,
) -> dict[str, Any]:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            raise SmokeFailure(
                f"daemon exited before health was ready ({daemon.returncode})\n"
                f"{daemon_log.read_text(errors='replace')}"
            )
        try:
            status, _, body = request(opener, f"{base_url}/api/health")
            if status == 200:
                return json.loads(body)
        except (OSError, json.JSONDecodeError):
            pass
        time.sleep(0.05)
    raise SmokeFailure(
        f"daemon health did not become ready\n{daemon_log.read_text(errors='replace')}"
    )


def verify_fake_media_boundaries(fake_directory: Path, root: Path, environment: dict[str, str]) -> None:
    source = root / "fake-source.mp4"
    recording = root / "fake-recording.mkv"
    output = root / "fake-output.mp4"
    source.write_bytes(b"source bytes")
    run(
        [
            str(fake_directory / "gpu-screen-recorder"),
            "-w",
            "screen",
            "-f",
            "60",
            "-o",
            str(recording),
        ],
        environment,
    )
    require(
        recording.read_bytes() == b"fake recorded media\n",
        "fake recorder boundary did not write media",
    )
    run(
        [str(fake_directory / "ffmpeg"), "-nostdin", "-i", str(source), str(output)],
        environment,
    )
    require(output.read_bytes() == b"fake encoded media\n", "fake ffmpeg boundary did not write media")
    probe = run(
        [str(fake_directory / "ffprobe"), "-v", "error", str(output)],
        environment,
    )
    probe_json = json.loads(probe.stdout)
    require(probe_json["streams"][0]["codec_name"] == "h264", "fake ffprobe codec drifted")


def verify_doctor(
    binary: Path,
    data_directory: Path,
    cfg_directory: Path,
    environment: dict[str, str],
) -> None:
    result = run(
        [
            str(binary),
            "doctor",
            "--json",
            "--data-dir",
            str(data_directory),
            "--cs2-cfg-dir",
            str(cfg_directory),
        ],
        environment,
    )
    report = json.loads(result.stdout)
    checks = {check["id"]: check["status"] for check in report["checks"]}
    for check_id in ("data_directory", "demo_import", "gsi_config"):
        require(checks.get(check_id) == "ready", f"Doctor check {check_id} was not ready: {checks}")
    require(
        checks.get("capture") in ("ready", "warning"),
        f"Doctor capture dependencies were blocked: {checks}",
    )
    require(
        checks.get("manual_flag") == "blocked",
        "headless Doctor must not claim user approval for a global shortcut",
    )


def configure_gsi(
    binary: Path,
    data_directory: Path,
    cfg_directory: Path,
    steam_id: str,
    environment: dict[str, str],
) -> str:
    result = run(
        [
            str(binary),
            "setup-gsi",
            "--data-dir",
            str(data_directory),
            "--cs2-cfg-dir",
            str(cfg_directory),
            "--steam-id",
            steam_id,
        ],
        environment,
    )
    token_path = data_directory / "gsi-token"
    steam_id_path = data_directory / "local-steam-id"
    token = token_path.read_text().strip()
    require(token and token not in result.stdout, "setup-gsi leaked its private token")
    require(steam_id_path.read_text().strip() == steam_id, "setup-gsi stored the wrong Steam ID")
    require(stat.S_IMODE(token_path.stat().st_mode) == 0o600, "GSI token is not private")
    require(stat.S_IMODE(steam_id_path.stat().st_mode) == 0o600, "local Steam ID is not private")
    config = (cfg_directory / "gamestate_integration_openfrag.cfg").read_text()
    require('"uri" "http://127.0.0.1:7130/gsi/router"' in config, "GSI config is not loopback-only")
    require("https://" not in config, "GSI config contains a remote endpoint")
    return token


def verify_mounted_api_contracts(
    opener: urllib.request.OpenerDirector,
    base_url: str,
    data_directory: Path,
) -> None:
    status, _, body = request(opener, f"{base_url}/api/setup")
    require(status == 200, f"setup API returned {status}: {body!r}")
    setup = json.loads(body)
    checks = {check["id"]: check["status"] for check in setup.get("checks", [])}
    require(
        checks
        == {
            "storage": "ready",
            "gsi": "ready",
            "local_identity": "ready",
            "capture": "needs_action",
            "ffprobe": "ready",
            "test_capture": "not_tested",
            "demo_import": "ready",
            "manual_flag": "unavailable",
            "background": "needs_action",
        },
        f"unexpected setup API response: {setup}",
    )
    require(
        setup.get("fingerprint", "").startswith("setup-"),
        "setup response lacks a fingerprint",
    )
    require(setup.get("complete") is False, "unfinished first run was marked complete")
    require(
        setup.get("cs2_cfg_candidates"),
        "setup omitted the discovered CS2 cfg directory",
    )

    for route, name in (("/api/matches", "matches"), ("/api/clips", "clips")):
        status, _, body = request(opener, f"{base_url}{route}")
        require(status == 200, f"{name} API returned {status}: {body!r}")
        require(json.loads(body) == [], f"fresh {name} collection is not empty")

    status, _, body = request(opener, f"{base_url}/api/diagnostics")
    require(status == 200, f"diagnostics API returned {status}: {body!r}")
    diagnostics = json.loads(body)
    require(
        diagnostics == {"capture": "needs_action", "gsi": "ready"},
        f"unexpected diagnostics API response: {diagnostics}",
    )

    status, _, body = request(opener, f"{base_url}/api/manual-flag", method="POST")
    require(status == 503, f"unconfigured Manual Flag returned {status}: {body!r}")
    manual_flag = json.loads(body)
    require(
        manual_flag == {"message": "capture pipeline is not connected"},
        f"Manual Flag was not explicitly unconfigured: {manual_flag}",
    )

    boundary = "openfrag-headless-e2e-boundary"
    multipart = (
        f"--{boundary}\r\n"
        'Content-Disposition: form-data; name="demo"; filename="invalid.dem"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
        "not a Source 2 Demo\r\n"
        f"--{boundary}--\r\n"
    ).encode()
    status, _, body = request(
        opener,
        f"{base_url}/api/imports",
        method="POST",
        body=multipart,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    require(status == 400, f"invalid multipart Demo returned {status}: {body!r}")
    import_error = json.loads(body)
    require(
        import_error.get("message", "").startswith("local Demo rejected:"),
        f"invalid multipart Demo was not explicitly rejected: {import_error}",
    )
    incoming = data_directory / "incoming"
    require(
        not incoming.exists() or not any(incoming.iterdir()),
        "invalid multipart Demo remained in the staging directory",
    )


def verify_http_contracts(
    opener: urllib.request.OpenerDirector,
    base_url: str,
    fixture_path: Path,
    data_directory: Path,
    token: str,
    steam_id: str,
) -> None:
    status, headers, dashboard = request(opener, f"{base_url}/")
    require(status == 200, f"dashboard returned {status}")
    require(headers.get("content-type", "").startswith("text/html"), "dashboard is not HTML")
    dashboard_text = dashboard.decode("utf-8")
    for marker in ("Tonight", "Import a local Demo", "Local match evidence and clip review."):
        require(marker in dashboard_text, f"dashboard is missing {marker!r}")
    require("https://" not in dashboard_text and "http://" not in dashboard_text, "dashboard has a remote asset")
    verify_mounted_api_contracts(opener, base_url, data_directory)

    fixture = json.loads(fixture_path.read_text())
    fixture["auth"]["token"] = token
    fixture["player"]["steamid"] = steam_id
    content_type = {"Content-Type": "application/json"}

    wrong_token = copy.deepcopy(fixture)
    wrong_token["auth"]["token"] = "wrong-private-token"
    status, _, body = request(
        opener,
        f"{base_url}/gsi/router",
        method="POST",
        body=json.dumps(wrong_token).encode(),
        headers=content_type,
    )
    require((status, body) == (401, b"unauthorized"), "GSI accepted a wrong token")

    wrong_identity = copy.deepcopy(fixture)
    wrong_identity["player"]["steamid"] = "76561198099999999"
    status, _, body = request(
        opener,
        f"{base_url}/gsi/router",
        method="POST",
        body=json.dumps(wrong_identity).encode(),
        headers=content_type,
    )
    require((status, body) == (401, b"unauthorized"), "GSI accepted a wrong local identity")

    payload = json.dumps(fixture, separators=(",", ":")).encode()
    status, _, body = request(
        opener,
        f"{base_url}/gsi/router",
        method="POST",
        body=payload,
        headers=content_type,
    )
    require((status, body) == (200, b"seeded"), f"valid GSI seed failed: {status} {body!r}")
    status, _, body = request(
        opener,
        f"{base_url}/gsi/router",
        method="POST",
        body=payload,
        headers=content_type,
    )
    require((status, body) == (200, b"duplicate"), "duplicate GSI snapshot was not idempotent")

    artifacts = [path.read_bytes() for path in (data_directory / "artifacts" / "sha256").rglob("*") if path.is_file()]
    require(artifacts, "GSI did not persist sanitized evidence")
    retained = b"\n".join(artifacts)
    for secret in (token, steam_id, "de_e2e_secret", "weapon_e2e_secret"):
        require(secret.encode() not in retained, f"sanitized evidence retained {secret!r}")
    require(b'"presence"' in retained, "sanitized evidence is missing presence metadata")


def smoke(binary: Path) -> None:
    repository = Path(__file__).resolve().parents[2]
    fixture_path = repository / "tests/e2e/fixtures/gsi-snapshot.json"
    fake_directory = repository / "tests/e2e/fakes"
    require(binary.is_file(), f"daemon binary does not exist: {binary}")
    require(fixture_path.is_file(), f"GSI fixture does not exist: {fixture_path}")
    for fake in ("gpu-screen-recorder", "ffmpeg", "ffprobe"):
        require((fake_directory / fake).is_file(), f"fake boundary does not exist: {fake}")

    with tempfile.TemporaryDirectory(prefix="openfrag-headless-e2e-") as temporary:
        root = Path(temporary)
        data_directory = root / "data"
        cfg_directory = root / "cfg"
        runtime_directory = root / "runtime"
        for directory in (data_directory, cfg_directory, runtime_directory):
            directory.mkdir()
        (runtime_directory / "pipewire-0").touch()
        environment = os.environ.copy()
        environment["PATH"] = f"{fake_directory}{os.pathsep}{environment.get('PATH', '')}"
        environment["XDG_RUNTIME_DIR"] = str(runtime_directory)
        environment["XDG_SESSION_TYPE"] = "wayland"
        environment["OPENFRAG_E2E_FAKE_LOG"] = str(root / "fake-tools.log")
        for desktop_variable in ("DISPLAY", "WAYLAND_DISPLAY", "BROWSER"):
            environment.pop(desktop_variable, None)

        verify_fake_media_boundaries(fake_directory, root, environment)
        verify_doctor(binary, data_directory, cfg_directory, environment)
        steam_id = "76561198000000001"
        token = configure_gsi(binary, data_directory, cfg_directory, steam_id, environment)

        port = reserve_loopback_port()
        base_url = f"http://127.0.0.1:{port}"
        daemon_log = root / "daemon.log"
        daemon: subprocess.Popen[bytes] | None = None
        with daemon_log.open("wb") as log:
            try:
                daemon = subprocess.Popen(
                    [
                        str(binary),
                        "serve",
                        "--bind",
                        f"127.0.0.1:{port}",
                        "--data-dir",
                        str(data_directory),
                    ],
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                health = wait_for_health(opener, base_url, daemon, daemon_log)
                require(health.get("binding") == "loopback", f"unexpected health binding: {health}")
                require(health.get("database") == "ready", f"database is not ready: {health}")
                require(health.get("gsi") == "ready", f"GSI is not ready: {health}")
                require(health.get("telemetry") is False, f"telemetry unexpectedly enabled: {health}")
                require(health.get("upload_path") is False, f"upload path unexpectedly enabled: {health}")
                verify_http_contracts(
                    opener,
                    base_url,
                    fixture_path,
                    data_directory,
                    token,
                    steam_id,
                )
            finally:
                if daemon is not None and daemon.poll() is None:
                    daemon.terminate()
                    try:
                        daemon.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        daemon.kill()
                        daemon.wait(timeout=5)
                        raise SmokeFailure("daemon did not terminate within five seconds")
        require(
            daemon is not None and daemon.returncode in (0, -signal.SIGTERM),
            f"daemon shutdown returned {None if daemon is None else daemon.returncode}",
        )
        print("headless smoke: PASS")
        print(
            "covered: doctor, setup-gsi, health, dashboard APIs, invalid import, "
            "GSI auth, dedup, sanitization, shutdown"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    arguments = parser.parse_args()
    binary = arguments.binary.resolve()
    try:
        smoke(binary)
    except (SmokeFailure, OSError, json.JSONDecodeError) as error:
        print(f"headless smoke: FAIL: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

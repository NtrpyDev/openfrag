#!/usr/bin/env python3
"""Prove the complete local product through an installed canonical static bundle."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import tarfile
import tempfile
import time
import urllib.request

from headless_smoke import (
    SmokeFailure,
    configure_gsi,
    request,
    require,
    reserve_loopback_port,
    run,
    wait_for_health,
)


APPLICATION_ID = "io.github.ntrpydev.openfrag"
STEAM_ID = "76561197964020430"


def write_live_recorder(path: Path) -> None:
    path.write_text(
        """#!/bin/sh
set -eu
output=
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    output=$1
  fi
  shift
done
[ -n "$output" ]
mkdir -p "$output"
save() {
  destination="$output/manual-$(date +%s%N).mkv"
  printf '%s\n' 'installed acceptance media' > "$destination"
}
trap save USR1
trap 'exit 0' INT TERM
while :; do sleep 0.05; done
"""
    )
    path.chmod(0o755)


def install_bundle(repository: Path, binary: Path, root: Path, environment: dict[str, str]) -> Path:
    release = root / "release"
    extract = root / "extract"
    release.mkdir()
    extract.mkdir()
    run(
        [str(repository / "packaging/build-release-bundle.sh"), str(binary), str(release)],
        environment,
    )
    archive = release / "openfrag-v1.0.0-linux-x86_64.tar.gz"
    require(archive.is_file(), "canonical static archive was not created")
    with tarfile.open(archive, "r:gz") as bundle:
        bundle.extractall(extract, filter="data")
    unpacked = extract / "openfrag-v1.0.0-linux-x86_64"
    run([str(unpacked / "packaging/install-user.sh"), "--staged"], environment)
    installed = root / "home/.local/bin/openfragd"
    require(installed.read_bytes() == binary.read_bytes(), "installed daemon differs from the bundle")
    desktop = root / f"home/.local/share/applications/{APPLICATION_ID}.desktop"
    service = root / f"home/.config/systemd/user/app-{APPLICATION_ID}.service"
    require(desktop.is_file(), "canonical desktop entry was not installed")
    require(service.is_file(), "canonical user service was not installed")
    require(stat.S_IMODE(installed.stat().st_mode) == 0o755, "installed daemon mode drifted")
    return installed


def multipart_demo() -> tuple[bytes, dict[str, str]]:
    boundary = "openfrag-installed-mvp-boundary"
    body = (
        f"--{boundary}\r\n"
        'Content-Disposition: form-data; name="demo"; filename="selected.dem"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode() + b"PBDEMS2\0private installed fixture\n" + f"\r\n--{boundary}--\r\n".encode()
    return body, {"Content-Type": f"multipart/form-data; boundary={boundary}"}


def json_request(
    opener: urllib.request.OpenerDirector,
    url: str,
    *,
    method: str = "GET",
    payload: dict[str, object] | None = None,
) -> tuple[int, bytes]:
    headers = {"Content-Type": "application/json"} if payload is not None else None
    body = json.dumps(payload).encode() if payload is not None else None
    status, _, response = request(opener, url, method=method, body=body, headers=headers)
    return status, response


def wait_for_collection(
    opener: urllib.request.OpenerDirector,
    url: str,
    expected: int,
) -> list[dict[str, object]]:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        status, body = json_request(opener, url)
        if status == 200:
            collection = json.loads(body)
            if len(collection) == expected:
                return collection
        time.sleep(0.05)
    raise SmokeFailure(f"{url} did not reach {expected} records")


def assert_loopback_only(daemon: subprocess.Popen[bytes], port: int) -> None:
    socket_inodes = set()
    for descriptor in Path(f"/proc/{daemon.pid}/fd").iterdir():
        try:
            target = os.readlink(descriptor)
        except OSError:
            continue
        if target.startswith("socket:["):
            socket_inodes.add(target.removeprefix("socket:[").removesuffix("]"))
    owned = []
    for table_name in ("tcp", "tcp6"):
        table = Path(f"/proc/{daemon.pid}/net/{table_name}")
        for line in table.read_text().splitlines()[1:]:
            columns = line.split()
            if columns[9] in socket_inodes:
                owned.append((table_name, columns[1], columns[2], columns[3]))
    expected_port = f"{port:04X}"
    require(
        any(name == "tcp" and local == f"0100007F:{expected_port}" and state == "0A" for name, local, _, state in owned),
        f"daemon did not own the expected loopback listener: {owned}",
    )
    for table_name, local, remote, _ in owned:
        local_address = local.split(":", 1)[0]
        remote_address = remote.split(":", 1)[0]
        allowed = {"0100007F", "00000000"} if table_name == "tcp" else {"00000000000000000000000001000000", "0" * 32}
        require(local_address in allowed, f"daemon opened a non-loopback local socket: {owned}")
        require(remote_address in allowed, f"daemon opened a remote socket: {owned}")


def exercise_dashboard(
    opener: urllib.request.OpenerDirector,
    base_url: str,
    data_directory: Path,
    token: str,
) -> None:
    status, headers, dashboard = request(opener, f"{base_url}/")
    require(status == 200, "installed dashboard did not load")
    require(headers.get("content-type", "").startswith("text/html"), "dashboard was not HTML")
    require(b"https://" not in dashboard and b"http://" not in dashboard, "dashboard has a remote asset")

    demo, multipart_headers = multipart_demo()
    status, _, body = request(
        opener,
        f"{base_url}/api/imports",
        method="POST",
        body=demo,
        headers=multipart_headers,
    )
    require(status == 200, f"selected Demo import failed: {status} {body!r}")
    job = json.loads(body)
    require(job["status"] == "completed" and job["progress"] == "100%", f"import was incomplete: {job}")

    matches = wait_for_collection(opener, f"{base_url}/api/matches", 1)
    require(matches[0]["map"] == "de_fixture", f"Match map was not explicit: {matches}")
    require(matches[0]["rating_state"] == "rated", f"Match was not rated: {matches}")
    match_id = str(matches[0]["id"])
    status, body = json_request(opener, f"{base_url}/api/matches/{match_id}")
    require(status == 200, f"Match detail failed: {status} {body!r}")
    detail = json.loads(body)
    require(detail["rating_state"] == "rated" and detail["rating"], f"Rating was fabricated or absent: {detail}")
    require(len(detail["receipts"]) >= 6, f"Rating did not expose component Receipts: {detail}")
    for receipt_reference in detail["receipts"]:
        status, receipt_body = json_request(opener, f"{base_url}/api/receipts/{receipt_reference['id']}")
        require(status == 200, f"Receipt lookup failed: {status} {receipt_body!r}")
        receipt = json.loads(receipt_body)
        require(receipt["metric_key"], f"Receipt lacked an evidence key: {receipt}")

    status, body = json_request(opener, f"{base_url}/api/manual-flag", method="POST")
    require(status == 200, f"Manual Flag failed: {status} {body!r}")
    clips = wait_for_collection(opener, f"{base_url}/api/clips", 1)
    clip_id = str(clips[0]["id"])
    status, body = json_request(
        opener,
        f"{base_url}/api/clips/{clip_id}",
        method="PATCH",
        payload={"title": "Installed keeper", "note": "Private local review", "tags": ["manual"], "decision": "keep"},
    )
    require(status == 200, f"Clip review failed: {status} {body!r}")

    status, _, preview = request(opener, f"{base_url}/api/clips/{clip_id}/preview")
    require(status == 200 and preview == b"installed acceptance media\n", "source Clip preview drifted")
    status, body = json_request(
        opener,
        f"{base_url}/api/clips/{clip_id}/trim",
        method="POST",
        payload={"start_ms": 1_000, "end_ms": 5_000},
    )
    require(status == 200, f"Clip trim failed: {status} {body!r}")
    trim = json.loads(body)
    require((trim["start_ms"], trim["end_ms"]) == (1_000, 5_000), f"trim bounds drifted: {trim}")
    derived_id = str(trim["clip_id"])
    require(derived_id != clip_id, f"trim did not create a derived Clip: {trim}")
    status, _, preview = request(opener, f"{base_url}/api/clips/{derived_id}/preview")
    require(status == 200 and preview == b"fake encoded media\n", "trimmed Clip preview drifted")
    status, body = json_request(opener, f"{base_url}/api/clips/{derived_id}/export", method="POST")
    require(status == 200, f"Clip export failed: {status} {body!r}")
    exported = Path(json.loads(body)["path"])
    require(exported.is_file() and exported.is_relative_to(data_directory / "exports"), "export escaped private storage")

    status, diagnostics_body = json_request(opener, f"{base_url}/api/diagnostics")
    require(status == 200, f"diagnostics failed: {status} {diagnostics_body!r}")
    diagnostics_text = diagnostics_body.decode()
    for secret in (token, STEAM_ID, str(data_directory)):
        require(secret not in diagnostics_text, f"diagnostics exposed private value {secret!r}")


def installed_mvp(binary: Path) -> None:
    repository = Path(__file__).resolve().parents[2]
    require(binary.is_file() and os.access(binary, os.X_OK), f"acceptance binary is unavailable: {binary}")
    with tempfile.TemporaryDirectory(prefix="openfrag-installed-mvp-") as temporary:
        root = Path(temporary)
        home = root / "home"
        data_directory = root / "data"
        cfg_directory = root / "cfg"
        output_directory = root / "capture"
        runtime_directory = root / "runtime"
        host_directory = root / "host"
        for directory in (home, data_directory, cfg_directory, output_directory, runtime_directory, host_directory):
            directory.mkdir()
        (runtime_directory / "pipewire-0").touch()
        recorder = host_directory / "gpu-screen-recorder"
        write_live_recorder(recorder)
        fake_directory = repository / "tests/e2e/fakes"
        shutil.copy2(fake_directory / "ffmpeg", host_directory / "ffmpeg")
        ffprobe = host_directory / "ffprobe"
        ffprobe.write_text(
            "#!/bin/sh\n"
            "printf '%s\\n' '{\"streams\":[{\"codec_type\":\"video\"}],"
            "\"format\":{\"duration\":\"30.000\"}}'\n"
        )
        ffprobe.chmod(0o755)
        environment = os.environ.copy()
        environment.update(
            {
                "HOME": str(home),
                "XDG_CONFIG_HOME": str(home / ".config"),
                "XDG_DATA_HOME": str(home / ".local/share"),
                "XDG_RUNTIME_DIR": str(runtime_directory),
                "XDG_SESSION_TYPE": "wayland",
                "PATH": f"{host_directory}{os.pathsep}{environment.get('PATH', '')}",
                "OPENFRAG_ACCEPTANCE_FIXTURE": "complete-rating-v1",
                "OPENFRAG_E2E_FAKE_LOG": str(root / "host-tools.log"),
            }
        )
        for variable in ("DISPLAY", "WAYLAND_DISPLAY", "BROWSER", "http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY"):
            environment.pop(variable, None)
        installed = install_bundle(repository, binary, root, environment)
        token = configure_gsi(installed, data_directory, cfg_directory, STEAM_ID, environment)
        run(
            [
                str(installed),
                "setup-capture",
                "--data-dir",
                str(data_directory),
                "--recorder",
                "native",
                "--recorder-path",
                str(recorder),
                "--capture-target",
                "screen",
                "--output-dir",
                str(output_directory),
                "--ffprobe-path",
                str(host_directory / "ffprobe"),
                "--enabled",
                "true",
            ],
            environment,
        )

        port = reserve_loopback_port()
        base_url = f"http://127.0.0.1:{port}"
        daemon_log = root / "daemon.log"
        daemon: subprocess.Popen[bytes] | None = None
        with daemon_log.open("wb") as log:
            try:
                daemon = subprocess.Popen(
                    [str(installed), "serve", "--bind", f"127.0.0.1:{port}", "--data-dir", str(data_directory)],
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                health = wait_for_health(opener, base_url, daemon, daemon_log)
                require(health["binding"] == "loopback", f"installed binding drifted: {health}")
                require(health["telemetry"] is False and health["upload_path"] is False, f"local-only contract drifted: {health}")
                assert_loopback_only(daemon, port)
                try:
                    exercise_dashboard(opener, base_url, data_directory, token)
                except Exception:
                    log.flush()
                    print(daemon_log.read_text(errors="replace"), file=os.sys.stderr)
                    host_log = root / "host-tools.log"
                    if host_log.is_file():
                        print(host_log.read_text(errors="replace"), file=os.sys.stderr)
                    raise
            finally:
                if daemon is not None and daemon.poll() is None:
                    daemon.terminate()
                    try:
                        daemon.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        daemon.kill()
                        daemon.wait(timeout=5)
                        raise SmokeFailure("installed daemon did not stop within five seconds")
        require(daemon is not None and daemon.returncode in (0, -signal.SIGTERM), "installed daemon shutdown failed")
    print("installed MVP journey: PASS")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        installed_mvp(arguments.binary.resolve())
    except (SmokeFailure, OSError, json.JSONDecodeError) as error:
        print(f"installed MVP journey: FAIL: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

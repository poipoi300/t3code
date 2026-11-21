#!/usr/bin/env python3
"""Run the two live preview tests on private servers with a deterministic CDP edge."""

import argparse
import base64
import json
import os
from pathlib import Path
import selectors
import shutil
import sqlite3
import subprocess
import tempfile
import time


HERE = Path(__file__).resolve().parent
WORKER_FIXTURE = HERE.parents[2] / "t3-preview" / "driver" / "tests" / "worker-fixture.cjs"
VIEWPORT_JPEG = (HERE / "prv_2.jpg").read_bytes()

PLAYWRIGHT = """
const {chromium} = require(FIXTURE);
const launch = chromium.launch;
chromium.launch = async (...options) => {
  const browser = await launch(...options);
  const create = browser.newContext;
  browser.newContext = async (...options) => {
    const context = await create(...options);
    const connect = context.newCDPSession;
    context.newCDPSession = async (...options) => {
      const session = await connect(...options);
      const send = session.send;
      session.send = async (method, params) => {
        if (method === 'Page.captureScreenshot') return {data: JPEG};
        if (method === 'Page.startScreencast') {
          queueMicrotask(() => session.emit('Page.screencastFrame', {
            data: JPEG,
            sessionId: 1,
            metadata: {deviceWidth: 1280, deviceHeight: 800},
          }));
        }
        return send(method, params);
      };
      return session;
    };
    return context;
  };
  return browser;
};
chromium.launchPersistentContext = async (_directory, options) =>
  (await chromium.launch(options)).newContext();
exports.chromium = chromium;
"""


def pairing_url(server):
    assert server.stdout is not None
    # Read bytes directly so the readiness check also sees complete lines already buffered.
    seen = b""
    deadline = time.monotonic() + 20
    with selectors.DefaultSelector() as selector:
        selector.register(server.stdout, selectors.EVENT_READ)
        while time.monotonic() < deadline and len(seen) < 32768:
            if not selector.select(max(0, deadline - time.monotonic())):
                break
            chunk = os.read(server.stdout.fileno(), 4096)
            if not chunk:
                break
            seen += chunk
            for line in seen.split(b"\n")[:-1]:
                if line.startswith(b"Pairing URL: "):
                    return line.removeprefix(b"Pairing URL: ").decode()
    raise RuntimeError("private preview server did not issue a pairing URL")


def run(width, args):
    with tempfile.TemporaryDirectory(prefix="preview-live-", dir=args.scratch) as private:
        root = Path(private)
        server_dir = root / "server"
        executable = server_dir / "tools/chrome-headless-shell/linux64/157.0.8103.78/chrome-headless-shell"
        executable.parent.mkdir(parents=True)
        executable.write_text("#!/bin/sh\nexit 1\n")
        executable.chmod(0o700)
        module = root / "playwright.cjs"
        module.write_text(
            PLAYWRIGHT.replace("FIXTURE", json.dumps(str(WORKER_FIXTURE))).replace(
                "JPEG", json.dumps(base64.b64encode(VIEWPORT_JPEG).decode())
            )
        )
        env = {
            "PATH": os.environ["PATH"],
            "XDG_RUNTIME_DIR": str(root),
            "T3_PREVIEW_PLAYWRIGHT_MODULE": str(module),
            "T3CODE_PORT_DISCOVERY": "0",
            "T3_UI_ENGINE_PANICS": "fail",
        }
        with (root / "server.stderr").open("wb") as stderr:
            server = subprocess.Popen(
                [args.server, "serve", "--no-browser", "--host", "127.0.0.1", "--port", "0",
                 "--base-dir", server_dir, "--home-dir", root],
                cwd=root, env=env, stdout=subprocess.PIPE, stderr=stderr,
            )
            try:
                url = pairing_url(server)
                with sqlite3.connect(server_dir / "userdata/statev2.sqlite") as conn:
                    conn.execute("""
                        INSERT INTO orchestration_v2_projection_threads
                          (thread_id, project_id, title, default_provider, runtime_mode,
                           interaction_mode, created_at, updated_at, payload_json)
                        VALUES ('thread', 'project', 'Preview test', 'codex', 'full-access',
                                'default', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '{}')
                    """)
                info = root / "test-info.json"
                with os.fdopen(os.open(info, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), "w") as out:
                    json.dump({"clientDir": str(root / "client"), "pairingUrl": url}, out)
                env["T3_PREVIEW_TEST_INFO"] = str(info)
                return subprocess.run(
                    [args.tests, "--ignored", "--exact", f"prv_2::annotation_highlights_live_viewer_at_{width}_width"],
                    cwd=root, env=env, timeout=45, check=False,
                ).returncode
            finally:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--tests", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    args = parser.parse_args()
    if shutil.which("node") is None:
        parser.error("the worker fixture requires a Node executable on PATH")
    outcomes = [run(width, args) for width in ("desktop", "phone")]
    raise SystemExit(max(outcomes))

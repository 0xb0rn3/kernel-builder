#!/usr/bin/env python3
"""Non-destructive regression checks. Compile on host; execute only in a VM."""
import fcntl
import hashlib
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
import unittest


class HardeningTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = Path(os.environ["AKT_BINARY"]).resolve(strict=True)
        cls.deny_setfd = Path(os.environ["AKT_DENY_SETFD"]).resolve(strict=True)
        print("binary_sha256=" + hashlib.sha256(cls.binary.read_bytes()).hexdigest(), flush=True)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="akt-hardening-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        # Do not open a desktop/browser while testing the loopback server.
        for name in ("xdg-open", "sensible-browser"):
            path = bin_dir / name
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o700)
        self.env = dict(os.environ, XDG_RUNTIME_DIR=str(self.runtime),
                        PATH=str(bin_dir) + os.pathsep + os.environ["PATH"], LC_ALL="C")
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            self.port = probe.getsockname()[1]
        self.server = subprocess.Popen(
            [str(self.binary), "gui", "--port", str(self.port)], env=self.env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(self.stop_server)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if self.server.poll() is not None:
                self.fail("GUI exited during startup")
            try:
                with socket.create_connection(("127.0.0.1", self.port), timeout=0.1):
                    return
            except OSError:
                time.sleep(0.05)
        self.fail("GUI did not start within five seconds")

    def stop_server(self):
        if self.server.poll() is None:
            self.server.terminate()
            try:
                self.server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait(timeout=3)

    def request(self, method="GET", path="/", extra="", host=None):
        host = host if host is not None else f"127.0.0.1:{self.port}"
        with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
            sock.sendall(f"{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n".encode())
            data = bytearray()
            while chunk := sock.recv(65536):
                data.extend(chunk)
        head, body = bytes(data).split(b"\r\n\r\n", 1)
        return int(head.split()[1]), body

    def test_setfd_failure_aborts_before_preparation_and_releases_lock(self):
        work = self.root / "work"
        env = dict(self.env, AKT_WORK=str(work), AKT_MAX_JOBS="0")
        # Invalid resource cap also prevents the old implementation reaching a recipe.
        # The fixed implementation must fail earlier, at the injected F_SETFD failure.
        result = subprocess.run([str(self.deny_setfd), str(self.binary), "build"],
                                env=env, text=True, capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Operation not permitted", result.stderr)
        self.assertEqual(sorted(p.name for p in work.iterdir()), [".akt-build.lock"])
        with (work / ".akt-build.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_normal_page_and_rejected_build_requests(self):
        status, body = self.request()
        self.assertEqual(status, 200)
        self.assertNotIn(b"__AKT_TOKEN__", body)
        self.assertIsNotNone(re.search(rb"[0-9a-f]{32}", body))
        self.assertEqual(self.request("POST", "/api/build")[0], 403)
        self.assertEqual(self.request("POST", "/api/build", "X-Akt-Token: wrong\r\n")[0], 403)
        self.assertEqual(self.request(host="foreign.invalid")[0], 421)
        self.assertEqual(self.request("POST", "/api/build", "Origin: null\r\n")[0], 403)

    def test_duplicate_security_headers(self):
        for header in ("Host", "Origin", "X-Akt-Token"):
            with self.subTest(header=header):
                extra = f"{header}: a\r\n{header}: b\r\n"
                self.assertEqual(self.request("POST", "/api/build", extra)[0], 400)

    def test_log_read_bounds_and_special_file_refusals(self):
        log = self.runtime / "akt-build.log"
        log.write_bytes(b"a" * (300 * 1024))
        self.assertEqual(len(self.request(path="/api/log")[1]), 256 * 1024)
        log.unlink()
        secret = self.root / "sentinel"
        secret.write_text("inert-private-sentinel")
        log.symlink_to(secret)
        self.assertEqual(self.request(path="/api/log")[1], b"")
        log.unlink()
        os.mkfifo(log)
        self.assertEqual(self.request(path="/api/log")[1], b"")
        log.unlink()
        log.write_text("inert-log")
        self.runtime.chmod(0o777)
        try:
            self.assertEqual(self.request(path="/api/log")[1], b"")
        finally:
            self.runtime.chmod(0o700)
        self.assertEqual(secret.read_text(), "inert-private-sentinel")

    def check_slow_request(self, prefix):
        with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
            sock.sendall(prefix)
            start = time.monotonic()
            sock.settimeout(0.2)
            response = b""
            while time.monotonic() - start < 25:
                try:
                    sock.sendall(b"x")
                except OSError:
                    break
                try:
                    response += sock.recv(4096)
                    if response:
                        break
                except socket.timeout:
                    pass
                time.sleep(0.2)
            elapsed = time.monotonic() - start
        self.assertIn(b"408 Request Timeout", response)
        self.assertLess(elapsed, 24)
        self.assertEqual(self.request()[0], 200, "server still handles normal requests")

    def test_absolute_deadline_on_request_line(self):
        self.check_slow_request(b"GET /")

    def test_absolute_deadline_on_header(self):
        self.check_slow_request(f"GET / HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\nX-Slow: ".encode())


if __name__ == "__main__":
    expected = os.environ.get("ARXOS_TEST_VM_UUID", "").lower()
    actual = Path("/sys/class/dmi/id/product_uuid").read_text().strip().lower()
    if not expected or actual != expected:
        raise SystemExit("refusing: explicit disposable-VM UUID does not match")
    if subprocess.run(["systemd-detect-virt", "--vm", "--quiet"]).returncode:
        raise SystemExit("refusing: not a virtual machine")
    print("vm_uuid=" + actual, flush=True)
    unittest.main(verbosity=2)

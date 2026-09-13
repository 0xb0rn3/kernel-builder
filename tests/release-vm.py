#!/usr/bin/env python3
"""CLI, archive and installation checks; execute only on the named disposable VM."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(os.environ["AKT_RELEASE_DIR"]).resolve()
VERSION = "0.1.1"


class ReleaseTests(unittest.TestCase):
    def test_checksums(self):
        lines = (ROOT / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(lines), 4)
        for line in lines:
            digest, name = line.split()
            self.assertEqual(hashlib.sha256((ROOT / name).read_bytes()).hexdigest(), digest)

    def check_cli(self, binary):
        def run(*args):
            return subprocess.run([binary, *args], text=True, capture_output=True, check=True,
                                  env=dict(os.environ, AKT_WORK="/mnt/kernelbuild", AKT_MAX_JOBS="1"), timeout=15)
        self.assertEqual(run("--version").stdout.strip(), "akt " + VERSION)
        self.assertIn("resources", run("--help").stdout)
        self.assertTrue(json.loads(run("detect", "--json").stdout)["supported"])
        self.assertEqual(json.loads(run("resources", "--json").stdout)["jobs"], 1)
        self.assertEqual(json.loads(run("plan", "--json").stdout)["distro"], "arch")
        self.assertIn("no build performed", run("build", "--dry-run").stdout)
        self.assertNotEqual(subprocess.run([binary, "gate", "/nonexistent/release-test-config"], capture_output=True).returncode, 0)

    def test_bare_binaries(self):
        for abi in ("linux", "linux-musl"):
            with self.subTest(abi=abi):
                self.check_cli(ROOT / ("akt-x86_64-" + abi))

    def test_bundles(self):
        for abi in ("linux", "linux-musl"):
            with self.subTest(abi=abi), tempfile.TemporaryDirectory() as tmp:
                with tarfile.open(ROOT / f"kernel-builder-v{VERSION}-x86_64-{abi}.tar.gz") as archive:
                    for member in archive.getmembers():
                        p = Path(member.name)
                        self.assertFalse(p.is_absolute())
                        self.assertNotIn("..", p.parts)
                        self.assertFalse(member.issym() or member.islnk() or member.isdev())
                        self.assertEqual(p.parts[0], "kernel-builder")
                    archive.extractall(tmp, filter="data")
                bundle = Path(tmp) / "kernel-builder"
                for name in ("akt", "install.sh", "README.md", "config/arxos.fragment", "ui/index.html", "patches/arxos/README.md", "patches/kali/README.md"):
                    self.assertTrue((bundle / name).is_file(), name)
                self.assertEqual((bundle / "akt").read_bytes(), (ROOT / ("akt-x86_64-" + abi)).read_bytes())
                subprocess.run(["bash", "-n", bundle / "install.sh"], check=True)
                self.check_cli(bundle / "akt")

    def test_install_bundles_in_disposable_guest(self):
        self.assertEqual(os.geteuid(), 0, "guest installation test needs root")
        for abi in ("linux", "linux-musl"):
            with self.subTest(abi=abi), tempfile.TemporaryDirectory() as tmp:
                with tarfile.open(ROOT / f"kernel-builder-v{VERSION}-x86_64-{abi}.tar.gz") as archive:
                    archive.extractall(tmp, filter="data")
                bundle = Path(tmp) / "kernel-builder"
                subprocess.run(["bash", bundle / "install.sh"], check=True, timeout=30)
                installed = Path("/usr/local/bin/akt")
                self.assertEqual(installed.read_bytes(), (bundle / "akt").read_bytes())
                for directory in ("config", "patches", "ui"):
                    for path in (bundle / directory).rglob("*"):
                        if path.is_file():
                            target = Path("/usr/share/kernel-builder") / path.relative_to(bundle)
                            self.assertEqual(target.read_bytes(), path.read_bytes())
                self.assertIn("Exec=akt gui", Path("/usr/share/applications/kernel-builder.desktop").read_text())
                self.check_cli(installed)


if __name__ == "__main__":
    if Path("/sys/class/dmi/id/product_uuid").read_text().strip().lower() != "2193f46f-cd6a-41bb-b29e-9a57d425b36d":
        raise SystemExit("refusing non-throwaway machine")
    unittest.main(verbosity=2)

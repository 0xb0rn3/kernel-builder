#!/usr/bin/env python3
"""Run in a throwaway VM only, with host-compiled binaries. No kernel build."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parents[1]
HELPER = Path(os.environ["RESOURCE_HELPER"]).resolve()
AKT = Path(os.environ["AKT_BINARY"]).resolve()
PRIVATE = Path(os.environ["PRIVATE_BUILDER"]).resolve()


class ResourcesTests(unittest.TestCase):
    def test_shared_source_is_identical(self):
        self.assertEqual((HERE / "src/resources.rs").read_bytes(), (PRIVATE / "resources.rs").read_bytes())

    def test_live_plan_uses_mounted_work_disk(self):
        env = dict(os.environ, AKT_WORK="/mnt/kernelbuild", AKT_MAX_JOBS="1")
        r = subprocess.run([AKT, "resources", "--json"], env=env, text=True, capture_output=True, check=True)
        data = json.loads(r.stdout)
        self.assertEqual(data["jobs"], 1)
        self.assertEqual(data["thinlto_jobs"], 1)
        self.assertGreaterEqual(data["disk_free_bytes"], 60 * 1024**3)
        dry = subprocess.run([AKT, "build", "--dry-run"], env=env, text=True, capture_output=True)
        self.assertEqual(dry.returncode, 0, dry.stderr)
        self.assertIn("no build performed", dry.stdout)

    def test_bad_job_cap_rejected(self):
        for value in ("0", "-1", "garbage"):
            r = subprocess.run([AKT, "resources", "--json"], env=dict(os.environ, AKT_WORK="/mnt/kernelbuild", AKT_MAX_JOBS=value), capture_output=True)
            self.assertNotEqual(r.returncode, 0)

    def test_recipe_executes_with_capped_make_jobs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            recipe = root / "PKGBUILD"
            recipe.write_text('make() { printf "%s\\n" "$*"; }\nmake -j"$(nproc)" all\nmake -j"$(nproc)" modules\n')
            subprocess.run([HELPER, "recipe", recipe], check=True)
            r = subprocess.run(["bash", recipe], env=dict(os.environ, ARXOS_BUILD_JOBS="3"), text=True, capture_output=True, check=True)
            self.assertEqual(r.stdout.splitlines(), ["-j3 all", "-j3 modules"])
            env = dict(os.environ)
            env.pop("ARXOS_BUILD_JOBS", None)
            self.assertNotEqual(subprocess.run(["bash", recipe], env=env, capture_output=True).returncode, 0)

    def test_real_retained_makefile_preserves_flags(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src = PRIVATE / "build/arxos/cos/linux-cachyos/src/cachyos-7.2.0-1"
            original = (src / "Makefile").read_bytes()
            (root / "Makefile").write_bytes(original)
            (root / ".config").write_text("CONFIG_LTO_CLANG_THIN=y\n")
            subprocess.run([HELPER, "kernel", root, "2"], check=True)
            changed = (root / "Makefile").read_bytes()
            self.assertEqual(changed.replace(b"# AKT bounded ThinLTO\nKBUILD_LDFLAGS += --thinlto-jobs=2\n", b""), original)
            (root / "Makefile").write_bytes(original)
            (root / ".config").write_text("# CONFIG_LTO_CLANG_THIN is not set\n")
            subprocess.run([HELPER, "kernel", root, "2"], check=True)
            self.assertEqual((root / "Makefile").read_bytes(), original)

    def test_existing_entrypoints_and_wrapper(self):
        for name in ("build.sh", "build-host.sh"):
            subprocess.run(["bash", "-n", PRIVATE / name], check=True)
        wrapper = (PRIVATE / "build-host.sh").read_text()
        self.assertNotIn("sed -i", wrapper)
        self.assertNotIn("taskset", wrapper)
        engine = (HERE / "src/build.rs").read_text()
        self.assertNotIn("$(nproc)", engine)
        self.assertIn("resources::limit_recipe", engine)
        self.assertEqual(engine.count("resources::limit_kernel"), 2)


if __name__ == "__main__":
    actual = Path("/sys/class/dmi/id/product_uuid").read_text().strip().lower()
    expected = os.environ["ARXOS_TEST_VM_UUID"].lower()
    if actual != expected or expected != "2193f46f-cd6a-41bb-b29e-9a57d425b36d":
        raise SystemExit("refusing non-throwaway machine")
    unittest.main(verbosity=2)

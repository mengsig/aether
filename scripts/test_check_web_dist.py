"""scripts/check_web_dist.sh against a throwaway repository holding a web/dist/."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent / "check_web_dist.sh"


def git(root, *args):
    subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True)


class CheckWebDistTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "scripts").mkdir()
        shutil.copy(SCRIPT, self.root / "scripts" / SCRIPT.name)
        dist = self.root / "web" / "dist"
        (dist / "assets").mkdir(parents=True)
        (dist / "index.html").write_text("<!doctype html>\n")
        (dist / "assets" / "index-abc.js").write_text("console.log(1)\n")
        git(self.root, "init", "-q")
        git(self.root, "add", ".")
        git(self.root, "-c", "user.name=test", "-c", "user.email=test@example.invalid",
            "commit", "-q", "-m", "fixture")

    def check(self):
        return subprocess.run(["sh", str(self.root / "scripts" / SCRIPT.name)],
                              capture_output=True, text=True, timeout=30)

    def test_an_unchanged_bundle_passes(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("matches a fresh build (2 files)", result.stdout)

    def test_a_changed_file_fails_and_is_named(self):
        (self.root / "web" / "dist" / "index.html").write_text("<!doctype html><p>new\n")
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("web/dist/index.html", result.stderr)

    def test_a_new_file_fails(self):
        (self.root / "web" / "dist" / "assets" / "index-def.js").write_text("console.log(2)\n")
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("web/dist/assets/index-def.js", result.stderr)

    def test_a_file_the_build_no_longer_writes_fails(self):
        (self.root / "web" / "dist" / "assets" / "index-abc.js").unlink()
        result = self.check()
        self.assertEqual(result.returncode, 1)
        self.assertIn("web/dist/assets/index-abc.js", result.stderr)

    def test_a_missing_bundle_is_a_usage_error(self):
        shutil.rmtree(self.root / "web" / "dist")
        result = self.check()
        self.assertEqual(result.returncode, 2)
        self.assertIn("build first", result.stderr)


if __name__ == "__main__":
    unittest.main()

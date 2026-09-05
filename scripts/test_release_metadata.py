import unittest
import tempfile
from pathlib import Path
from unittest.mock import patch
from release_metadata import msi_version, validate, validate_checks


class ReleaseVersionTests(unittest.TestCase):
    def test_channels_and_builds_upgrade_in_order(self):
        versions = ["0.1.0-alpha.1", "0.1.0-alpha.2", "0.1.0-beta.1", "0.1.0-beta.2", "0.1.0-rc.1", "0.1.0-rc.2", "0.1.0"]
        numeric = [tuple(map(int, msi_version(v).split("."))) for v in versions]
        self.assertTrue(all(before < after for before, after in zip(numeric, numeric[1:])))
        # Both legacy installers (0.0.0.N / 0.0.1.N) are older than current beta/RC packages.
        self.assertLess((0, 0, 1), numeric[2])

    def test_final_versions_and_windows_limits(self):
        self.assertEqual(msi_version("0.1.0"), "0.1.0")
        self.assertEqual(msi_version("1.2.3"), "1.2.3")
        for invalid in ["v0.1.0", "0.1.0-beta.0", "0.1.0-beta.10000", "0.2.0-rc.1", "256.0.0", "0.0.65536", "0.1.0+1"]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                msi_version(invalid)


class ExistingCiTests(unittest.TestCase):
    def test_existing_ci_must_pass_and_latest_rerun_wins(self):
        checks = [{"name": name, "id": index, "conclusion": "success"} for index, name in enumerate(
            ["rust", "Tests (macOS)", "Tests (Windows)", "Tests (Ubuntu)", "release-metadata"])]
        validate_checks({"check_runs": checks})
        with self.assertRaisesRegex(ValueError, "Wait for successful CI"):
            validate_checks({"check_runs": []})
        checks.append({"name": "rust", "id": 100, "conclusion": "failure"})
        with self.assertRaisesRegex(ValueError, "rust"):
            validate_checks({"check_runs": checks})


class SourceIdentityTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "packaging/wix").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        (self.root / "Cargo.lock").write_text('[[package]]\nname = "remcmd-core"\nversion = "0.1.0"\n')
        (self.root / "packaging/wix/windows.toml").write_text('version = "0.1.0"\n')

    @patch("release_metadata.subprocess.check_output")
    def test_recovery_must_build_the_requested_tag(self, git):
        git.side_effect = ["new-commit\n", "tagged-commit\n"]
        with self.assertRaisesRegex(ValueError, "Checkout does not match"):
            validate(self.root, "v0.1.0")

    @patch("release_metadata.subprocess.check_output", return_value="same-commit\n")
    def test_wrong_tag_or_stale_lockfile_is_rejected(self, git):
        with self.assertRaisesRegex(ValueError, "does not match Cargo"):
            validate(self.root, "v0.1.1")
        (self.root / "Cargo.lock").write_text('[[package]]\nname = "remcmd-core"\nversion = "0.0.9"\n')
        with self.assertRaisesRegex(ValueError, "stale version"):
            validate(self.root)

    @patch("release_metadata.subprocess.check_output", return_value="same-commit\n")
    def test_matching_tag_and_source_produce_identity(self, git):
        self.assertEqual(validate(self.root, "v0.1.0")["commit"], "same-commit")


if __name__ == "__main__":
    unittest.main()

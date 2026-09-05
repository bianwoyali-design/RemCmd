"""Validate a release's source, Cargo lockfile and MSI upgrade version."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tomllib


def msi_version(version):
    match = re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(alpha|beta|rc)\.([1-9]\d*))?", version)
    if not match:
        raise ValueError(f"Unsupported release version: {version}")
    major, minor, patch = map(int, match.group(1, 2, 3))
    channel, number = match.group(4, 5)
    if channel:
        if (major, minor, patch) != (0, 1, 0) or int(number) > 9999:
            raise ValueError("Define an MSI upgrade mapping before releasing this prerelease series")
        major, minor, patch = 0, 0, {"alpha": 0, "beta": 10000, "rc": 20000}[channel] + int(number)
    if major > 255 or minor > 255 or patch > 65535:
        raise ValueError("Version exceeds Windows Installer ProductVersion limits")
    return f"{major}.{minor}.{patch}"


def validate(root, expected_tag=""):
    root = Path(root)
    cargo = tomllib.loads((root / "Cargo.toml").read_text())
    version = cargo["workspace"]["package"]["version"]
    expected_msi = msi_version(version)
    wix = tomllib.loads((root / "packaging/wix/windows.toml").read_text())
    if wix["version"] != expected_msi:
        raise ValueError(f"MSI version must be {expected_msi}, got {wix['version']}")
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    for package in lock["package"]:
        if package["name"].startswith("remcmd-") and "source" not in package and package["version"] != version:
            raise ValueError(f"Cargo.lock has stale version for {package['name']}")
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    if expected_tag:
        if expected_tag != f"v{version}":
            raise ValueError(f"Tag {expected_tag} does not match Cargo version v{version}")
        tagged_commit = subprocess.check_output(
            ["git", "rev-parse", "--verify", f"refs/tags/{expected_tag}^{{commit}}"], cwd=root, text=True
        ).strip()
        if tagged_commit != commit:
            raise ValueError("Checkout does not match the release tag commit")
    return {"version": version, "msi_version": expected_msi, "commit": commit}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".")
    parser.add_argument("--expected-tag", default="")
    parser.add_argument("--github-output")
    args = parser.parse_args()
    metadata = validate(args.root, args.expected_tag)
    if args.github_output:
        with open(args.github_output, "a", encoding="utf-8") as output:
            for key, value in metadata.items():
                output.write(f"{key}={value}\n")
    print(json.dumps(metadata, indent=2))

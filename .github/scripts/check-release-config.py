#!/usr/bin/env python3
"""Fail if release-plz.toml does not put every workspace member in the
"ferrocord" version group (all crates share the workspace version, so a member
outside the group could be bumped on its own)."""

import json
import subprocess
import sys
import tomllib
from pathlib import Path

GROUP = "ferrocord"
root = Path(__file__).resolve().parents[2]
meta = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))
members = {p["name"] for p in meta["packages"]}
config = tomllib.loads((root / "release-plz.toml").read_text(encoding="utf-8"))
packages = {p["name"]: p for p in config.get("package", [])}

errors = []
for name in sorted(members):
    if packages.get(name, {}).get("version_group") != GROUP:
        errors.append(f"{name}: add a [[package]] entry with version_group = \"{GROUP}\"")
for name in sorted(set(packages) - members):
    errors.append(f"{name}: listed in release-plz.toml but not a workspace member")
include = set(packages.get("ferrocord", {}).get("changelog_include", []))
for name in sorted(members - include - {"ferrocord"}):
    errors.append(f"{name}: add it to changelog_include of the ferrocord package")

if errors:
    print("release-plz.toml is out of sync with the workspace:", *errors, sep="\n  ", file=sys.stderr)
    sys.exit(1)
print(f"release-plz.toml covers all {len(members)} workspace members")

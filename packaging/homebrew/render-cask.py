#!/usr/bin/env python3
"""Render the Homebrew cask for a release from its two macOS disk images.

Usage::

    python3 packaging/homebrew/render-cask.py --version 0.2.0 --repository LB45440078L/Ferrocord \\
        --arm64-dmg dist/ferrocord-0.2.0-macos-arm64.dmg \\
        --x86_64-dmg dist/ferrocord-0.2.0-macos-x86_64.dmg \\
        [--notarized] --output Casks/ferrocord.rb

The cask is published to the project's tap (``brew install --cask lb45440078l/tap/ferrocord``)
by the release workflow; see docs/RELEASING.md. Without ``--notarized`` the cask tells
users how to approve the first launch, since Gatekeeper blocks unnotarized apps.
"""

from __future__ import annotations

import argparse
import hashlib
import re
from pathlib import Path

TEMPLATE = Path(__file__).resolve().with_name("ferrocord.rb.in")

UNNOTARIZED_CAVEATS = '''
  caveats <<~EOS
    This build of Ferrocord is not notarized by Apple. The first time you open it,
    macOS will refuse; approve it once in System Settings > Privacy & Security >
    "Open Anyway".
  EOS
'''


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--repository", required=True, help="GitHub owner/name hosting the releases")
    ap.add_argument("--arm64-dmg", required=True, type=Path)
    ap.add_argument("--x86_64-dmg", required=True, type=Path)
    ap.add_argument("--notarized", action="store_true")
    ap.add_argument("--output", required=True, type=Path)
    args = ap.parse_args()

    for dmg, arch in ((args.arm64_dmg, "arm64"), (args.x86_64_dmg, "x86_64")):
        expected = f"ferrocord-{args.version}-macos-{arch}.dmg"
        if dmg.name != expected:
            raise SystemExit(f"{dmg} should be named {expected}: the cask URL is derived from it")

    cask = TEMPLATE.read_text(encoding="utf-8")
    for key, value in {
        "@VERSION@": args.version,
        "@REPOSITORY@": args.repository,
        "@SHA256_ARM64@": sha256(args.arm64_dmg),
        "@SHA256_X86_64@": sha256(args.x86_64_dmg),
        "@CAVEATS@": "" if args.notarized else UNNOTARIZED_CAVEATS,
    }.items():
        cask = cask.replace(key, value)
    leftover = re.findall(r"@[A-Z0-9_]+@", cask)
    if leftover:
        raise SystemExit(f"unfilled placeholders: {leftover}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(cask, encoding="utf-8", newline="\n")
    print(f"rendered {args.output}")


if __name__ == "__main__":
    main()

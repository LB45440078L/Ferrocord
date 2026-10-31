#!/usr/bin/env python3
"""Add the release being packaged to the AppStream metainfo <releases> list.

    python3 packaging/linux/stamp-metainfo.py <version> [YYYY-MM-DD]

Edits packaging/linux/dev.ferrocord.Ferrocord.metainfo.xml in place (the
release workflow runs it on its own checkout before packaging, so the
committed file does not have to be updated by hand for every release).
Does nothing if the version is already listed.
"""

import datetime
import re
import sys
from pathlib import Path

path = Path(__file__).resolve().parent / "dev.ferrocord.Ferrocord.metainfo.xml"
version = sys.argv[1]
date = sys.argv[2] if len(sys.argv) > 2 else datetime.date.today().isoformat()
if not re.fullmatch(r"\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", version):
    sys.exit(f"not a SemVer version: {version}")

text = path.read_text(encoding="utf-8")
if f'version="{version}"' in text:
    print(f"metainfo already lists {version}")
    sys.exit(0)
kind = "development" if "-" in version else "stable"
entry = (
    f'    <release version="{version}" date="{date}" type="{kind}">\n'
    f"      <url>https://github.com/LB45440078L/Ferrocord/releases/tag/v{version}</url>\n"
    "    </release>\n"
)
new, n = re.subn(r"(<releases>\n)", lambda m: m.group(1) + entry, text, count=1)
if n != 1:
    sys.exit("no <releases> element found")
path.write_text(new, encoding="utf-8", newline="\n")
print(f"added release {version} ({date}) to {path.name}")

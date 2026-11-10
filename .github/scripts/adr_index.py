#!/usr/bin/env python3
"""Write the ADR index from the ADR files, or check that it is up to date (--check)."""

import argparse
import re
import sys
from pathlib import Path

ADR_DIR = Path(__file__).resolve().parents[2] / "docs" / "adr"
MARKER = "<!-- index -->"
# ADRs 0001 to 0053 were numbered; every later one is named by its slug alone.
LAST_NUMBER = 53
NUMBERED = re.compile(r"^(\d{4})-[a-z0-9-]+\.md$")
SLUG = re.compile(r"^[a-z][a-z0-9-]*\.md$")


def entry(path):
    text = path.read_text()
    title = re.match(r"# (?:\d{4}\. )?(.+)", text)
    date = re.search(r"^- \*\*Date:\*\* (\d{4}-\d{2}-\d{2})", text, re.M)
    status = re.search(r"^- \*\*Status:\*\* (.+)$", text, re.M)
    if not (title and date and status):
        raise ValueError(f"{path.name}: needs a `# ` title, a **Date:** and a **Status:** line")
    return date[1], path.name, title[1].strip(), status[1].strip()


def problems(adr_dir):
    found = []
    for path in sorted(adr_dir.glob("*.md")):
        if path.name == "README.md":
            continue
        numbered = NUMBERED.match(path.name)
        if numbered and int(numbered[1]) > LAST_NUMBER:
            found.append(f"{path.name}: a new ADR has no number, name it `<slug>.md`")
        elif not numbered and not SLUG.match(path.name):
            found.append(f"{path.name}: name an ADR `<slug>.md`, in lower case with dashes")
    return found


def index(adr_dir):
    entries = sorted(
        entry(path) for path in adr_dir.glob("*.md") if path.name != "README.md"
    )
    lines = ["| Decision | Date | Status |", "|---|---|---|"]
    for date, name, title, status in entries:
        numbered = NUMBERED.match(name)
        label = f"{numbered[1]}. {title}" if numbered else title
        lines.append(f"| [{label}]({name}) | {date} | {status} |")
    return "\n".join(lines) + "\n"


def readme_with_index(adr_dir):
    readme = (adr_dir / "README.md").read_text()
    head, marker, _ = readme.partition(MARKER)
    if not marker:
        raise ValueError(f"README.md has no {MARKER} line")
    return f"{head}{MARKER}\n\n{index(adr_dir)}"


def main(argv=None, adr_dir=ADR_DIR):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if the index is stale")
    args = parser.parse_args(argv)
    found = problems(adr_dir)
    readme = adr_dir / "README.md"
    wanted = readme_with_index(adr_dir)
    if args.check:
        if readme.read_text() != wanted:
            found.append("README.md: the index is stale, run .github/scripts/adr_index.py")
    else:
        readme.write_text(wanted)
    for problem in found:
        print(problem, file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())

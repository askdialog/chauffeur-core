"""Choose PR and merge queue checks; main and manual runs check everything."""

import json
import os
import re
import subprocess


def scope(paths):
    rust = javascript = False
    for path in paths:
        # These inputs cross the Rust/TypeScript boundary, including a Rust include_str!.
        if path.startswith("protocol-ts/") or path in {
            "sidecar/src/version.ts", "sidecar/src/package-crypto.ts",
            "sidecar/src/agents/context-files.ts",
        }:
            rust = javascript = True
        elif path.startswith("app/src-tauri/"):
            rust = javascript = True
        elif path.startswith(("app/", "sidecar/", "packages/remote/", "relay/", "slack/")):
            javascript = True
        elif path.startswith("docs/") or path in {
            "README.md", "CONTRIBUTING.md", "SECURITY.md", "CODE_OF_CONDUCT.md",
            "CODING_STANDARDS.md", "CLAUDE.md", "AGENTS.md", "LICENSE", "THIRD_PARTY_NOTICES.md",
        }:
            continue
        else:
            # New directories, build scripts and CI changes get full coverage by default.
            rust = javascript = True
    return {"rust": rust, "javascript": javascript}


def validate(needs):
    if needs["changes"]["result"] != "success":
        raise ValueError("Change detection or repository checks did not pass")
    for job in ("rust", "javascript"):
        expected = "success" if needs["changes"]["outputs"][job] == "true" else "skipped"
        if needs[job]["result"] != expected:
            raise ValueError(f"{job}: expected {expected}, got {needs[job]['result']}")


def select_checks(event, base, head):
    if event not in {"pull_request", "merge_group"}:
        return {"rust": True, "javascript": True}
    if any(not re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (base, head)):
        raise ValueError("Expected full Git commit IDs")
    separator = "..." if event == "pull_request" else ".."
    paths = subprocess.check_output([
        "git", "diff", "--name-only", "--no-renames", "-z", f"{base}{separator}{head}", "--",
    ]).decode(errors="surrogateescape").split("\0")
    return scope(path for path in paths if path)


def main():
    if "CHECK_NEEDS" in os.environ:
        validate(json.loads(os.environ["CHECK_NEEDS"]))
        return
    selected = select_checks(
        os.environ["GITHUB_EVENT_NAME"], os.environ["BASE_SHA"], os.environ["HEAD_SHA"]
    )
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        for name, enabled in selected.items():
            output.write(f"{name}={str(enabled).lower()}\n")
    print(selected)


if __name__ == "__main__":
    main()

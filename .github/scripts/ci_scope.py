"""Choose PR checks conservatively; main and manual runs check everything."""

import json
import os
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


def main():
    if "CHECK_NEEDS" in os.environ:
        validate(json.loads(os.environ["CHECK_NEEDS"]))
        return
    if os.environ["GITHUB_EVENT_NAME"] == "pull_request":
        base, head = os.environ["PR_BASE_SHA"], os.environ["PR_HEAD_SHA"]
        # Pass only object IDs to git, never user-controlled revisions or shell expressions.
        if any(len(sha) != 40 or any(c not in "0123456789abcdef" for c in sha) for sha in (base, head)):
            raise ValueError("Expected full Git commit IDs")
        paths = subprocess.check_output([
            "git", "diff", "--name-only", "--no-renames", "-z", f"{base}...{head}", "--",
        ]).decode().split("\0")
        selected = scope(path for path in paths if path)
    else:
        selected = {"rust": True, "javascript": True}
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        for name, enabled in selected.items():
            output.write(f"{name}={str(enabled).lower()}\n")
    print(selected)


if __name__ == "__main__":
    main()

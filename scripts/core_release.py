#!/usr/bin/env python3
"""Build identity and release verification for the public, extension-free edition."""
import argparse
import hashlib
import json
import re
import subprocess
import tomllib
from pathlib import Path

REPOSITORY = "askdialog/chauffeur-core"


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def version(root):
    value = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not re.fullmatch(r"0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError("Core must declare a stable 0.MINOR.PATCH version.")
    for file in ("app/package.json", "sidecar/package.json", "app/src-tauri/tauri.conf.json"):
        if json.loads((root / file).read_text())["version"] != value:
            raise ValueError(f"Synchronize the version in {file} before releasing Core.")
    for file, expected in (("sidecar/src/version.ts", f'SIDECAR_VERSION = "{value}"'),
                           ("app/src/state/app.ts", f'clientName: "chauffeur-app {value}"')):
        if expected not in (root / file).read_text():
            raise ValueError(f"Synchronize the version in {file} before releasing Core.")
    return value


def validate(root, expected=None):
    if (root / "distribution").exists() or (root / "extensions").exists():
        raise ValueError("Build Core from the public source checkout, without a private distribution or extensions.")
    actual = version(root)
    if expected and actual != expected:
        raise ValueError(f"Core source declares {actual}, not requested release {expected}.")
    return actual


def assets(value):
    if not re.fullmatch(r"0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError("Invalid Core version.")
    packages = [Path(f"target/release/Chauffeur-Core-{value}.dmg"),
                Path(f"dist/chauffeur-core-{value}-linux-x64.tar.gz"),
                Path(f"dist/chauffeur-core_{value}_ubuntu24.04_amd64.deb")]
    return [path for package in packages for path in (package, Path(f"{package}.sha256"))]


def verify_assets(value):
    paths = assets(value)
    for package, checksum in zip(paths[::2], paths[1::2]):
        if not package.is_file() or not package.stat().st_size:
            raise ValueError(f"Missing Core package: {package}")
        with package.open('rb') as file:
            digest = hashlib.file_digest(file, 'sha256').hexdigest()
        if checksum.read_text().strip() != f"{digest}  {package.name}":
            raise ValueError(f"Invalid Core checksum: {package}")
    return paths


def find_release(tag):
    """The release of `tag`, draft or not, or None.

    Looked up in the list of releases: GitHub's get-a-release-by-tag endpoint answers 404 for a
    draft, which is what every Core release is until this script publishes it.
    """
    pages = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{REPOSITORY}/releases?per_page=100"))
    return next((item for page in pages for item in page if item["tag_name"] == tag), None)


def publish(value, commit, notes_file):
    """Publish only verified Core artifacts, without becoming the legacy client's latest release."""
    paths = verify_assets(value)
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("Core publication requires the complete source commit.")
    source = run("gh", "api", f"repos/{REPOSITORY}/contents/Cargo.toml?ref={commit}",
                 "-H", "Accept: application/vnd.github.raw+json")
    if tomllib.loads(source)["workspace"]["package"]["version"] != value:
        raise ValueError("The public Core tag must point to sources declaring the release version.")
    tag = f"core-v{value}"
    existing = find_release(tag)
    refs = json.loads(run("gh", "api", f"repos/{REPOSITORY}/git/matching-refs/tags/{tag}"))
    exact = next((ref for ref in refs if ref["ref"] == f"refs/tags/{tag}"), None)
    if exact:
        target = json.loads(run("gh", "api", f"repos/{REPOSITORY}/commits/{tag}"))["sha"]
        if target != commit:
            raise ValueError("The Core tag already points to different sources; do not move published tags.")
    else:
        run("gh", "api", "--method", "POST", f"repos/{REPOSITORY}/git/refs",
            "-f", f"ref=refs/tags/{tag}", "-f", f"sha={commit}")
    if not existing:
        run("gh", "release", "create", tag, "--repo", REPOSITORY, "--target", commit,
            "--draft", "--latest=false", "--title", f"Chauffeur Core {value}",
            "--notes-file", str(notes_file), *map(str, paths))
    # Do not overwrite a published release or reuse somebody else's tag/draft.
    target = json.loads(run("gh", "api", f"repos/{REPOSITORY}/commits/{tag}"))["sha"]
    if target != commit:
        raise ValueError("The Core tag already points to different sources; do not move published tags.")
    uploaded = find_release(tag)
    if uploaded is None:
        raise ValueError(f"The Core release {tag} was not found after creating it.")
    indexed = {item["name"]: item for item in uploaded["assets"]}
    if set(indexed) != {path.name for path in paths}:
        raise ValueError("The Core release has missing or unexpected assets; repair the draft before publication.")
    for path in paths:
        with path.open('rb') as file:
            digest = hashlib.file_digest(file, 'sha256').hexdigest()
        if indexed[path.name].get('digest') != f"sha256:{digest}" or indexed[path.name]['size'] != path.stat().st_size:
            raise ValueError(f"Uploaded Core asset differs from the verified build: {path.name}")
    if uploaded['draft']:
        run("gh", "release", "edit", tag, "--repo", REPOSITORY, "--draft=false", "--latest=false")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--expect-version')
    args = parser.parse_args()
    print(validate(Path.cwd(), args.expect_version))

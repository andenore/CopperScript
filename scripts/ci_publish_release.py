"""Publish trusted tag build assets as an explicitly experimental prerelease.

Uses the GitHub REST API directly; no third-party release action, shell or
secret printing. Existing releases/assets are not silently overwritten.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import mimetypes
import os
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen


def request(url, token, payload=None, *, content_type="application/json"):
    encoded = json.dumps(payload).encode() if isinstance(payload, dict) else payload
    headers = {"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json",
               "X-GitHub-Api-Version": "2022-11-28", "Content-Type": content_type,
               "User-Agent": "CopperScript-board-inspection-release"}
    with urlopen(Request(url, data=encoded, headers=headers), timeout=120) as response:
        return json.load(response)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args(argv)
    ref = os.environ["GITHUB_REF"]
    if not ref.startswith("refs/tags/"):
        raise ValueError("publication requires a trusted tag event")
    tag = ref[len("refs/tags/"):]
    repo, token = os.environ["GITHUB_REPOSITORY"], os.environ["GH_TOKEN"]
    files = sorted(args.directory.iterdir())
    if not files or not (args.directory / "board-inspection-drafts.zip").is_file():
        raise ValueError("missing inspection bundle; refusing an empty release")
    if any(not path.is_file() or path.is_symlink() for path in files):
        raise ValueError("release inputs must be plain files")
    recorded = {}
    for line in (args.directory / "SHA256SUMS.txt").read_text(encoding="utf-8").splitlines():
        digest, name = line.split("  ", 1)
        if name in recorded or Path(name).name != name:
            raise ValueError("unsafe or duplicate checksum entry")
        recorded[name] = digest
    if set(recorded) != {path.name for path in files if path.name != "SHA256SUMS.txt"}:
        raise ValueError("release checksums do not cover the exact asset set")
    if any(hashlib.sha256(path.read_bytes()).hexdigest() != recorded[path.name]
           for path in files if path.name != "SHA256SUMS.txt"):
        raise ValueError("release asset checksum mismatch")
    endpoint = f"https://api.github.com/repos/{repo}/releases"
    try:
        request(endpoint + "/tags/" + quote(tag, safe=""), token)
    except HTTPError as error:
        if error.code != 404:
            raise
    else:
        raise ValueError("tag already has a release; refusing to overwrite published artifacts")
    body = (args.directory / "RELEASE-NOTES.md").read_text(encoding="utf-8")
    release = request(endpoint, token, {"tag_name": tag, "target_commitish": os.environ["GITHUB_SHA"],
        "name": f"{tag} — board inspection drafts", "body": body, "draft": True, "prerelease": True})
    upload = release["upload_url"].split("{", 1)[0]
    # Keep the release a private draft if any upload fails.
    for path in files:
        request(upload + "?name=" + quote(path.name, safe=""), token, path.read_bytes(),
                content_type=mimetypes.guess_type(path.name)[0] or "application/octet-stream")
    final_url = endpoint + f"/{release['id']}"
    # The HTTP helper POSTs when a payload is present; publication specifically PATCHes.
    payload = json.dumps({"draft": False}).encode()
    with urlopen(Request(final_url, data=payload, method="PATCH", headers={
        "Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28", "Content-Type": "application/json",
        "User-Agent": "CopperScript-board-inspection-release"}), timeout=120) as response:
        result = json.load(response)
    print(f"Published inspection prerelease: {result['html_url']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

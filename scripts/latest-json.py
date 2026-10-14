#!/usr/bin/env python3
"""Writes the app's update manifest (latest.json) for one release to stdout.

    scripts/latest-json.py --version 0.3.0 \\
        --url https://github.com/ckakkar/trav/releases/download/v0.3.0/Trav-0.3.0-macOS.app.tar.gz \\
        --signature-file Trav.app.tar.gz.sig [--notes notes.md]

The installed app reads it from the latest GitHub release. Apple Silicon and
Intel Macs both get the same universal build; the signature comes from
`tauri build` with the updater key, and the app refuses anything it can't verify.
"""

import argparse
import datetime
import json
import sys


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--version", required=True)
    p.add_argument("--url", required=True)
    p.add_argument("--signature-file", required=True)
    p.add_argument("--notes", help="release notes (Markdown)")
    a = p.parse_args()

    with open(a.signature_file, encoding="utf-8") as f:
        signature = f.read().strip()
    notes = ""
    if a.notes:
        with open(a.notes, encoding="utf-8") as f:
            notes = f.read().strip()
    build = {"signature": signature, "url": a.url}
    manifest = {
        "version": a.version,
        "notes": notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": {"darwin-aarch64": build, "darwin-x86_64": build},
    }
    json.dump(manifest, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()

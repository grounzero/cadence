#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Back up OpenBench database and media after writes have stopped."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import sqlite3
import subprocess
import tarfile
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--state-directory", type=Path, default=Path("/var/lib/cadence-openbench")
    )
    parser.add_argument(
        "--backup-directory", type=Path, default=Path("/var/backups/cadence-openbench")
    )
    parser.add_argument("--service", default="cadence-openbench.service")
    parser.add_argument(
        "--online",
        action="store_true",
        help="allow an active service; SQLite is consistent but media can race uploads",
    )
    args = parser.parse_args()

    active = subprocess.run(
        ["systemctl", "is-active", "--quiet", args.service], check=False
    ).returncode == 0
    if active and not args.online:
        parser.error(f"{args.service} is active; stop it or accept the media race with --online")

    state = args.state_directory.resolve()
    database = state / "db.sqlite3"
    if not database.is_file():
        parser.error(f"database does not exist: {database}")

    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    destination = args.backup_directory.resolve() / timestamp
    destination.mkdir(parents=True, mode=0o700, exist_ok=False)

    with sqlite3.connect(database) as source, sqlite3.connect(destination / "db.sqlite3") as target:
        source.backup(target)

    media = state / "media"
    if not media.is_dir():
        media = state / "Media"
    if media.is_dir():
        with tarfile.open(destination / "media.tar.gz", "w:gz") as archive:
            archive.add(media, arcname="media", recursive=True)

    manifest = {
        path.name: sha256(path)
        for path in sorted(destination.iterdir())
        if path.is_file()
    }
    (destination / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    for path in destination.iterdir():
        if path.is_file():
            path.chmod(0o600)
    print(destination)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

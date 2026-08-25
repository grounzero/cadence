#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Restore an OpenBench backup while retaining the displaced state."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path


CONFIRMATION = "restore-cadence-openbench"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def check_manifest(snapshot: Path) -> None:
    manifest_path = snapshot / "manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read {manifest_path}: {error}") from error
    if not isinstance(manifest, dict):
        raise RuntimeError(f"invalid manifest: {manifest_path}")
    for name, expected in manifest.items():
        path = snapshot / name
        actual = sha256(path) if path.is_file() else None
        if actual != expected:
            raise RuntimeError(f"backup checksum mismatch for {name}: {actual} != {expected}")


def check_service_stopped(service: str) -> None:
    result = subprocess.run(
        ["systemctl", "is-active", "--quiet", service],
        check=False,
    )
    if result.returncode == 0:
        raise RuntimeError(f"{service} is active; stop it before restoring")


def safe_extract(archive_path: Path, destination: Path) -> None:
    with tarfile.open(archive_path, "r:gz") as archive:
        root = destination.resolve()
        for member in archive.getmembers():
            target = (destination / member.name).resolve()
            if not target.is_relative_to(root):
                raise RuntimeError(f"unsafe backup path: {member.name}")
        archive.extractall(destination, filter="data")


def set_restored_owner(
    state: Path, database: Path, media: Path, user: str, group: str
) -> None:
    for path in [state, database, media, *media.rglob("*")]:
        if not path.is_symlink():
            shutil.chown(path, user=user, group=group)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path)
    parser.add_argument(
        "--state-directory", type=Path, default=Path("/var/lib/cadence-openbench")
    )
    parser.add_argument("--service", default="cadence-openbench.service")
    parser.add_argument("--service-user", default="openbench")
    parser.add_argument("--service-group", default="openbench")
    parser.add_argument("--confirm", required=True)
    args = parser.parse_args()
    if args.confirm != CONFIRMATION:
        parser.error(f"--confirm must be exactly {CONFIRMATION!r}")

    snapshot = args.snapshot.resolve()
    state = args.state_directory.resolve()
    check_service_stopped(args.service)
    check_manifest(snapshot)
    database = snapshot / "db.sqlite3"
    if not database.is_file():
        raise RuntimeError(f"snapshot has no database: {snapshot}")

    state.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".restore-", dir=state) as temporary_name:
        temporary = Path(temporary_name)
        shutil.copy2(database, temporary / "db.sqlite3")
        archive = snapshot / "media.tar.gz"
        if archive.is_file():
            safe_extract(archive, temporary)
        else:
            (temporary / "media").mkdir()

        timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        displaced = state / f"pre-restore-{timestamp}"
        displaced.mkdir()
        current_database = state / "db.sqlite3"
        current_media = state / "media"
        if current_database.exists():
            os.replace(current_database, displaced / "db.sqlite3")
        if current_media.exists():
            os.replace(current_media, displaced / "media")
        os.replace(temporary / "db.sqlite3", current_database)
        os.replace(temporary / "media", current_media)

    set_restored_owner(
        state,
        current_database,
        current_media,
        args.service_user,
        args.service_group,
    )

    print(f"Restored {snapshot}")
    print(f"Displaced state retained at {displaced}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

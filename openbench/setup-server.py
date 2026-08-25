#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Install Cadence configuration over a pinned official OpenBench server.

The default action prepares files but does not migrate data, start services, or
change nginx. Pass --activate only during an announced maintenance window.
"""

from __future__ import annotations

import argparse
import datetime
import getpass
import hashlib
import json
import os
import re
import secrets
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import urllib.parse
from pathlib import Path


SOURCE = Path(__file__).resolve().parent
PINS_PATH = SOURCE / "pins.json"
OVERLAY = SOURCE / "overlay"
DEPLOY = SOURCE / "deploy"


class SetupError(RuntimeError):
    """An installation error that should be displayed without a traceback."""


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Prepare a pinned official OpenBench server for Cadence."
    )
    value.add_argument("--prefix", type=Path, default=Path("/opt/cadence-openbench"))
    value.add_argument(
        "--config-directory", type=Path, default=Path("/etc/cadence-openbench")
    )
    value.add_argument(
        "--state-directory", type=Path, default=Path("/var/lib/cadence-openbench")
    )
    value.add_argument(
        "--backup-directory", type=Path, default=Path("/var/backups/cadence-openbench")
    )
    value.add_argument(
        "--import-state-directory",
        type=Path,
        help="existing OpenBench checkout/state containing db.sqlite3 and Media/",
    )
    value.add_argument("--service-user", default="openbench")
    value.add_argument("--service-group", default="openbench")
    value.add_argument(
        "--previous-service",
        default="openbench.service",
        help="service that must be stopped and will be disabled during first cutover",
    )
    value.add_argument(
        "--allowed-host",
        action="append",
        dest="allowed_hosts",
        help="accepted Host header; repeat for each name or address",
    )
    value.add_argument(
        "--csrf-origin",
        action="append",
        dest="csrf_origins",
        help="trusted HTTPS origin; repeat when using an HTTPS reverse proxy",
    )
    value.add_argument(
        "--behind-https-proxy",
        action="store_true",
        help="trust X-Forwarded-Proto and mark session and CSRF cookies secure",
    )
    value.add_argument(
        "--github-token-file",
        type=Path,
        help="file containing the private Cadence read token; never pass the token itself",
    )
    value.add_argument(
        "--install-nginx",
        action="store_true",
        help="install and enable the generated nginx site",
    )
    value.add_argument(
        "--listen",
        help="nginx listen address, for example 192.0.2.10:80; required with --install-nginx",
    )
    value.add_argument(
        "--server-name",
        action="append",
        dest="server_names",
        help="nginx server name; repeat as needed",
    )
    value.add_argument(
        "--activate",
        action="store_true",
        help="back up state, migrate, collect static files, and start the service",
    )
    value.add_argument(
        "--dry-run",
        action="store_true",
        help="print resolved paths and pins without writing or executing commands",
    )
    return value


def read_json(path: Path) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SetupError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise SetupError(f"{path} must contain a JSON object")
    return value


def pins() -> dict[str, object]:
    value = read_json(PINS_PATH)
    if value.get("schema") != 1:
        raise SetupError(f"unsupported pin manifest: {PINS_PATH}")
    for name in ("server", "client", "fastchess"):
        item = value.get(name)
        commit = item.get("commit") if isinstance(item, dict) else None
        if not isinstance(commit, str) or re.fullmatch(r"[0-9a-f]{40}", commit) is None:
            raise SetupError(f"the {name} pin is not a full lowercase commit ID")
    return value


def pin(name: str) -> dict[str, object]:
    value = pins().get(name)
    if not isinstance(value, dict):
        raise SetupError(f"pin manifest lacks {name}")
    return value


def run_checked(
    command: list[str],
    description: str,
    *,
    cwd: Path | None = None,
    environment: dict[str, str] | None = None,
) -> None:
    print(f"{description}...")
    try:
        subprocess.run(command, check=True, cwd=cwd, env=environment)
    except (OSError, subprocess.CalledProcessError) as error:
        raise SetupError(f"{description} failed: {error}") from error


def run_output(command: list[str], description: str, *, cwd: Path | None = None) -> str:
    try:
        result = subprocess.run(
            command,
            check=True,
            cwd=cwd,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise SetupError(f"{description} failed: {error}") from error
    return result.stdout.strip()


def file_sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    try:
        with path.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
    except OSError:
        return None
    return digest.hexdigest()


def secure_write(path: Path, content: str, mode: int = 0o640) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as output:
            output.write(content)
        temporary.chmod(mode)
        os.replace(temporary, path)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def ensure_root() -> None:
    if not hasattr(os, "geteuid") or os.geteuid() != 0:
        raise SetupError("server installation must run as root; use --dry-run to inspect it")


def validate_arguments(args: argparse.Namespace) -> None:
    account = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*")
    for label, value in (("service user", args.service_user), ("service group", args.service_group)):
        if account.fullmatch(value) is None:
            raise SetupError(f"invalid {label}: {value!r}")
    if re.fullmatch(r"[A-Za-z0-9_.@-]+\.service", args.previous_service) is None:
        raise SetupError(f"invalid previous service name: {args.previous_service!r}")
    for path in (
        args.prefix,
        args.config_directory,
        args.state_directory,
        args.backup_directory,
    ):
        if any(character.isspace() for character in str(path)):
            raise SetupError(f"deployment paths cannot contain whitespace: {path}")
    host = re.compile(r"[A-Za-z0-9_.:-]+")
    for value in (args.allowed_hosts or []) + (args.server_names or []):
        if host.fullmatch(value) is None:
            raise SetupError(f"invalid host or server name: {value!r}")
    for origin in args.csrf_origins or []:
        parsed = urllib.parse.urlparse(origin)
        if parsed.scheme != "https" or not parsed.netloc or parsed.path not in {"", "/"}:
            raise SetupError(f"CSRF origins must be absolute HTTPS origins: {origin!r}")
    if args.listen and re.fullmatch(r"(?:[A-Za-z0-9_.-]+|\[[0-9A-Fa-f:]+\]):[0-9]{1,5}", args.listen) is None:
        raise SetupError(f"invalid nginx listen address: {args.listen!r}")


def check_prerequisites(install_nginx: bool) -> None:
    required = ["getent", "git", "id", "python3", "runuser", "systemctl"]
    if install_nginx:
        required.append("nginx")
    missing = [program for program in required if shutil.which(program) is None]
    if missing:
        raise SetupError("missing server prerequisites: " + ", ".join(missing))


def service_is_active(name: str) -> bool:
    return subprocess.run(
        ["systemctl", "is-active", "--quiet", name],
        check=False,
    ).returncode == 0


def service_is_enabled(name: str) -> bool:
    return subprocess.run(
        ["systemctl", "is-enabled", "--quiet", name],
        check=False,
    ).returncode == 0


def stop_for_activation(previous_service: str) -> None:
    if previous_service != "cadence-openbench.service":
        if service_is_active(previous_service):
            raise SetupError(
                f"{previous_service} is still active; stop it and confirm uploads have finished"
            )
        if service_is_enabled(previous_service):
            run_checked(
                ["systemctl", "disable", previous_service],
                f"Disabling the previous OpenBench service {previous_service}",
            )
    if service_is_active("cadence-openbench.service"):
        run_checked(
            ["systemctl", "stop", "cadence-openbench.service"],
            "Stopping the current Cadence OpenBench service",
        )


def ensure_service_account(user: str, group: str) -> None:
    user_exists = subprocess.run(
        ["id", "-u", user], capture_output=True, check=False
    ).returncode == 0
    group_exists = subprocess.run(
        ["getent", "group", group], capture_output=True, check=False
    ).returncode == 0
    if not group_exists:
        groupadd = shutil.which("groupadd")
        if groupadd is None:
            raise SetupError(f"service group {group!r} is absent and groupadd is unavailable")
        run_checked([groupadd, "--system", group], f"Creating service group {group}")
    if user_exists:
        run_checked(["usermod", "-g", group, user], f"Assigning {user} to {group}")
        return
    useradd = shutil.which("useradd")
    if useradd is None:
        raise SetupError(f"service user {user!r} is absent and useradd is unavailable")
    run_checked(
        [useradd, "--system", "--gid", group, "--home-dir", "/nonexistent", "--shell", "/usr/sbin/nologin", user],
        f"Creating service account {user}",
    )


def verify_checkout(checkout: Path) -> None:
    expected = str(pin("server")["commit"])
    if not (checkout / ".git").is_dir():
        raise SetupError(f"{checkout} is not an OpenBench Git checkout")
    actual = run_output(["git", "rev-parse", "HEAD"], "Reading server commit", cwd=checkout)
    if actual != expected:
        raise SetupError(f"server checkout is {actual}, expected {expected}: {checkout}")
    changed = run_output(
        ["git", "status", "--porcelain", "--untracked-files=no"],
        "Checking the official server worktree",
        cwd=checkout,
    )
    unexpected = [line for line in changed.splitlines() if not line.endswith(" Config/config.json")]
    if unexpected:
        raise SetupError(
            "tracked files changed outside the external configuration link:\n"
            + "\n".join(unexpected)
        )
    expected_requirements = pin("server").get("requirements_sha256")
    actual_requirements = file_sha256(checkout / "requirements.txt")
    if actual_requirements != expected_requirements:
        raise SetupError(
            "the server dependency declaration differs from the reviewed tree: "
            f"expected {expected_requirements}, found {actual_requirements}"
        )


def ensure_checkout(prefix: Path) -> Path:
    server = pin("server")
    commit = str(server["commit"])
    checkout = prefix / "checkouts" / f"OpenBench-{commit}"
    if checkout.exists():
        verify_checkout(checkout)
        return checkout

    checkout.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=".OpenBench-new-", dir=checkout.parent))
    try:
        run_checked(["git", "init", str(temporary)], "Creating the server checkout")
        run_checked(
            ["git", "remote", "add", "origin", str(server["repository"])],
            "Setting the official OpenBench remote",
            cwd=temporary,
        )
        run_checked(
            ["git", "fetch", "--depth", "1", "origin", commit],
            f"Fetching official OpenBench {commit}",
            cwd=temporary,
        )
        run_checked(
            ["git", "checkout", "--detach", "FETCH_HEAD"],
            "Checking out the reviewed server commit",
            cwd=temporary,
        )
        verify_checkout(temporary)
        os.replace(temporary, checkout)
    except Exception:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return checkout


def generated_openbench_config() -> str:
    value = read_json(OVERLAY / "Config" / "config.base.json")
    client = pin("client")
    fastchess = pin("fastchess")
    configured = {
        **value,
        "client_version": client["version"],
        "client_repo_url": client["repository"],
        "client_repo_ref": client["commit"],
        "fastchess_min_version": fastchess["minimum_version"],
        "fastchess_repo_url": fastchess["repository"],
        "fastchess_repo_ref": fastchess["commit"],
    }
    return json.dumps(configured, indent=4) + "\n"


def read_token(args: argparse.Namespace, destination: Path) -> str | None:
    environment_token = os.environ.get("CADENCE_GITHUB_TOKEN")
    if environment_token:
        return environment_token.strip()
    if args.github_token_file:
        try:
            return args.github_token_file.read_text(encoding="utf-8").splitlines()[0].strip()
        except (OSError, IndexError) as error:
            raise SetupError(f"cannot read {args.github_token_file}: {error}") from error
    if destination.is_file():
        return None
    if sys.stdin.isatty():
        token = getpass.getpass("GitHub token with read access to Cadence: ").strip()
        if token:
            return token
    raise SetupError(
        "the private engine needs a server GitHub token; use CADENCE_GITHUB_TOKEN "
        "or --github-token-file"
    )


def replace_with_symlink(path: Path, target: Path) -> None:
    if path.is_symlink() and path.resolve() == target.resolve():
        return
    if path.exists() or path.is_symlink():
        if path.is_dir() and not path.is_symlink():
            raise SetupError(f"refusing to replace an upstream directory with a link: {path}")
        else:
            path.unlink()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.symlink_to(target, target_is_directory=target.is_dir())


def install_configuration(
    args: argparse.Namespace, checkout: Path, config_directory: Path
) -> None:
    engine_directory = config_directory / "Engines"
    site_directory = config_directory / "CadenceSite"
    engine_directory.mkdir(parents=True, exist_ok=True)
    site_directory.mkdir(parents=True, exist_ok=True)

    secure_write(
        config_directory / "pins.json",
        PINS_PATH.read_text(encoding="utf-8"),
    )
    secure_write(config_directory / "config.json", generated_openbench_config())
    secure_write(
        engine_directory / "Cadence.json",
        (OVERLAY / "Engines" / "Cadence.json").read_text(encoding="utf-8"),
    )
    secure_write(
        site_directory / "__init__.py",
        (OVERLAY / "CadenceSite" / "__init__.py").read_text(encoding="utf-8"),
    )
    secure_write(
        site_directory / "settings.py",
        (OVERLAY / "CadenceSite" / "settings.py").read_text(encoding="utf-8"),
    )

    credential = config_directory / "credentials.cadence"
    token = read_token(args, credential)
    if token is not None:
        secure_write(credential, token.rstrip("\r\n") + "\n")

    replace_with_symlink(checkout / "Config" / "config.json", config_directory / "config.json")
    replace_with_symlink(checkout / "Config" / "credentials.cadence", credential)
    replace_with_symlink(checkout / "Engines" / "Cadence.json", engine_directory / "Cadence.json")
    replace_with_symlink(checkout / "CadenceSite", site_directory)


def environment_values(
    args: argparse.Namespace, config_directory: Path, state_directory: Path
) -> dict[str, str]:
    hosts = args.allowed_hosts or ["localhost", "127.0.0.1", "openbench"]
    return {
        "OPENBENCH_SECRET_KEY_FILE": str(config_directory / "django-secret-key"),
        "OPENBENCH_ALLOWED_HOSTS": ",".join(hosts),
        "OPENBENCH_DATABASE": str(state_directory / "db.sqlite3"),
        "OPENBENCH_MEDIA_ROOT": str(state_directory / "media"),
        "OPENBENCH_STATIC_ROOT": str(state_directory / "static"),
        "OPENBENCH_CSRF_TRUSTED_ORIGINS": ",".join(args.csrf_origins or []),
        "OPENBENCH_BEHIND_HTTPS_PROXY": "1" if args.behind_https_proxy else "0",
        "PYTHONDONTWRITEBYTECODE": "1",
    }


def environment_file(values: dict[str, str]) -> str:
    lines = []
    for name, value in values.items():
        if "\n" in value or "\r" in value:
            raise SetupError(f"newline in environment value {name}")
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        lines.append(f'{name}="{escaped}"')
    return "\n".join(lines) + "\n"


def process_environment(values: dict[str, str], checkout: Path) -> dict[str, str]:
    environment = {
        "PATH": os.environ.get("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin"),
        "LANG": os.environ.get("LANG", "C.UTF-8"),
    }
    environment.update(values)
    environment["PYTHONPATH"] = str(checkout)
    environment["DJANGO_SETTINGS_MODULE"] = "CadenceSite.settings"
    return environment


def import_existing_state(source: Path, destination: Path) -> None:
    source = source.resolve()
    database_source = source / "db.sqlite3"
    database_target = destination / "db.sqlite3"
    if not database_source.is_file():
        raise SetupError(f"the import source has no db.sqlite3: {source}")
    if database_target.exists():
        raise SetupError(f"refusing to overwrite existing database: {database_target}")
    media_source = source / "Media"
    if not media_source.is_dir():
        media_source = source / "media"
    media_target = destination / "media"
    if any(media_target.iterdir()):
        raise SetupError(f"refusing to merge into non-empty media directory: {media_target}")

    temporary = Path(tempfile.mkdtemp(prefix=".state-import-", dir=destination))
    try:
        database_pending = temporary / "db.sqlite3"
        with sqlite3.connect(database_source) as old, sqlite3.connect(database_pending) as new:
            old.backup(new)
        media_pending = temporary / "media"
        if media_source.is_dir():
            shutil.copytree(media_source, media_pending)
        else:
            media_pending.mkdir()
        os.replace(database_pending, database_target)
        media_target.rmdir()
        os.replace(media_pending, media_target)
    except Exception:
        database_target.unlink(missing_ok=True)
        if not media_target.exists():
            media_target.mkdir()
        raise
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


def run_as_user(
    user: str,
    command: list[str],
    description: str,
    *,
    cwd: Path,
    environment: dict[str, str],
) -> None:
    runuser = shutil.which("runuser")
    if runuser is None:
        raise SetupError("runuser is required to execute Django as the service account")
    env_command = ["/usr/bin/env"] + [f"{key}={value}" for key, value in environment.items()]
    run_checked(
        [runuser, "-u", user, "--", *env_command, *command],
        description,
        cwd=cwd,
    )


def install_python_environment(
    prefix: Path, checkout: Path, config_directory: Path
) -> tuple[Path, bool]:
    commit = str(pin("server")["commit"])
    venv = prefix / "venvs" / f"server-{commit}"
    python = venv / "bin" / "python"
    if not python.is_file():
        run_checked(["python3", "-m", "venv", str(venv)], "Creating server virtual environment")

    lock = config_directory / f"server-python-packages-{commit}.txt"
    created_lock = not lock.is_file()
    if lock.is_file():
        requirements = ["-r", str(lock)]
    else:
        requirements = ["-r", str(checkout / "requirements.txt"), "gunicorn"]
    run_checked(
        [str(python), "-m", "pip", "install", *requirements],
        "Installing server Python dependencies",
    )
    if not lock.is_file():
        resolved = run_output(
            [str(python), "-m", "pip", "freeze", "--all"],
            "Recording resolved server Python dependencies",
        )
        secure_write(lock, resolved + "\n")
    return venv, created_lock


def render(template: Path, values: dict[str, str]) -> str:
    content = template.read_text(encoding="utf-8")
    for name, value in values.items():
        content = content.replace(f"@@{name}@@", value)
    unresolved = sorted(set(re.findall(r"@@[A-Z_]+@@", content)))
    if unresolved:
        raise SetupError(f"unresolved template values in {template}: {', '.join(unresolved)}")
    return content


def forwarded_proto_source(behind_https_proxy: bool) -> str:
    # Only trust an incoming scheme when the deployment explicitly declares
    # that a trusted outer proxy terminates HTTPS.
    return "$http_x_forwarded_proto" if behind_https_proxy else "$scheme"


def install_service(
    args: argparse.Namespace,
    checkout: Path,
    venv: Path,
    config_directory: Path,
    state_directory: Path,
) -> Path:
    unit = render(
        DEPLOY / "cadence-openbench.service.in",
        {
            "SERVICE_USER": args.service_user,
            "SERVICE_GROUP": args.service_group,
            "STATE_DIRECTORY": str(state_directory),
            "CONFIG_DIRECTORY": str(config_directory),
            "CHECKOUT": str(checkout),
            "VENV": str(venv),
        },
    )
    destination = Path("/etc/systemd/system/cadence-openbench.service")
    secure_write(destination, unit, mode=0o644)
    run_checked(["systemctl", "daemon-reload"], "Reloading systemd")
    return destination


def install_nginx(args: argparse.Namespace, state_directory: Path) -> Path | None:
    if not args.install_nginx:
        return None
    if not args.listen:
        raise SetupError("--listen is required with --install-nginx")
    names = args.server_names or args.allowed_hosts
    if not names:
        raise SetupError("--server-name or --allowed-host is required with --install-nginx")
    site = render(
        DEPLOY / "nginx-openbench.conf.in",
        {
            "LISTEN": args.listen,
            "SERVER_NAMES": " ".join(names),
            "STATIC_DIRECTORY": str(state_directory / "static"),
            "FORWARDED_PROTO": forwarded_proto_source(args.behind_https_proxy),
        },
    )
    available = Path("/etc/nginx/sites-available/cadence-openbench")
    enabled = Path("/etc/nginx/sites-enabled/cadence-openbench")
    secure_write(available, site, mode=0o644)
    if not enabled.exists() and not enabled.is_symlink():
        enabled.symlink_to(available)
    listen_host = args.listen.rsplit(":", 1)[0] if ":" in args.listen else ""
    if listen_host and listen_host not in {"0.0.0.0", "*", "[::]"}:
        drop_in = Path("/etc/systemd/system/nginx.service.d/after-networking.conf")
        secure_write(
            drop_in,
            (DEPLOY / "nginx-after-networking.conf").read_text(encoding="utf-8"),
            mode=0o644,
        )
        run_checked(["systemctl", "daemon-reload"], "Reloading nginx unit configuration")
    run_checked(["nginx", "-t"], "Validating nginx configuration")
    return available


def backup_state(state_directory: Path, backup_directory: Path) -> Path | None:
    database = state_directory / "db.sqlite3"
    if not database.is_file():
        return None
    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    destination = backup_directory / timestamp
    destination.mkdir(parents=True, mode=0o700, exist_ok=False)
    with sqlite3.connect(database) as source, sqlite3.connect(destination / "db.sqlite3") as target:
        source.backup(target)
    media = state_directory / "media"
    if media.is_dir():
        with tarfile.open(destination / "media.tar.gz", "w:gz") as archive:
            archive.add(media, arcname="media", recursive=True)
    manifest = {}
    for path in sorted(destination.iterdir()):
        if path.is_file():
            manifest[path.name] = file_sha256(path)
    secure_write(destination / "manifest.json", json.dumps(manifest, indent=2) + "\n")
    for path in destination.iterdir():
        if path.is_file():
            path.chmod(0o600)
    return destination


def set_owner(path: Path, user: str, group: str, recursive: bool = False) -> None:
    paths = [path]
    if recursive and path.is_dir():
        paths.extend(path.rglob("*"))
    for item in paths:
        if not item.is_symlink():
            shutil.chown(item, user=user, group=group)


def show_plan(args: argparse.Namespace) -> None:
    server = pin("server")
    client = pin("client")
    fastchess = pin("fastchess")
    checkout = args.prefix.resolve() / "checkouts" / f"OpenBench-{server['commit']}"
    print("Dry run; no files, packages, services, or network connections were changed.")
    print(f"Server source:  {server['repository']} at {server['commit']}")
    print(f"Client source:  {client['repository']} at {client['commit']}")
    print(f"fastchess:      {fastchess['repository']} at {fastchess['commit']}")
    print(f"Checkout:       {checkout}")
    print(f"Configuration:  {args.config_directory.resolve()}")
    print(f"State:          {args.state_directory.resolve()}")
    print(f"Backups:        {args.backup_directory.resolve()}")
    print(f"Activate:       {args.activate}")
    print(f"Install nginx:  {args.install_nginx}")


def configure(args: argparse.Namespace) -> None:
    pins()
    validate_arguments(args)
    if args.import_state_directory and not args.activate:
        raise SetupError("--import-state-directory is only allowed with --activate")
    if args.dry_run:
        show_plan(args)
        return
    ensure_root()
    check_prerequisites(args.install_nginx)
    ensure_service_account(args.service_user, args.service_group)

    prefix = args.prefix.resolve()
    config_directory = args.config_directory.resolve()
    state_directory = args.state_directory.resolve()
    backup_directory = args.backup_directory.resolve()
    for path in (prefix, config_directory, state_directory, backup_directory):
        path.mkdir(parents=True, exist_ok=True)
    for path in (state_directory / "media", state_directory / "static"):
        path.mkdir(parents=True, exist_ok=True)

    checkout = ensure_checkout(prefix)
    install_configuration(args, checkout, config_directory)
    secret_key = config_directory / "django-secret-key"
    if not secret_key.is_file():
        secure_write(secret_key, secrets.token_urlsafe(64) + "\n")

    values = environment_values(args, config_directory, state_directory)
    secure_write(config_directory / "server.env", environment_file(values))
    venv, created_dependency_lock = install_python_environment(
        prefix, checkout, config_directory
    )
    unit = install_service(args, checkout, venv, config_directory, state_directory)
    nginx_site = install_nginx(args, state_directory)

    config_directory.chmod(0o750)
    for path in config_directory.rglob("*"):
        if path.is_file():
            path.chmod(0o640)
    set_owner(config_directory, "root", args.service_group, recursive=True)
    set_owner(state_directory, args.service_user, args.service_group, recursive=True)
    backup_directory.chmod(0o700)

    backup = None
    if args.activate:
        if created_dependency_lock:
            raise SetupError(
                "the server dependency lock was resolved for the first time; "
                "review it and rerun --activate to prove it can be reused"
            )
        stop_for_activation(args.previous_service)
        if args.import_state_directory:
            import_existing_state(args.import_state_directory, state_directory)
            set_owner(state_directory, args.service_user, args.service_group, recursive=True)
        backup = backup_state(state_directory, backup_directory)
        environment = process_environment(values, checkout)
        python = venv / "bin" / "python"
        run_as_user(
            args.service_user,
            [str(python), "manage.py", "migrate", "--noinput"],
            "Migrating the OpenBench database",
            cwd=checkout,
            environment=environment,
        )
        run_as_user(
            args.service_user,
            [str(python), "manage.py", "collectstatic", "--noinput"],
            "Collecting static files",
            cwd=checkout,
            environment=environment,
        )
        run_checked(
            ["systemctl", "enable", "--now", "cadence-openbench.service"],
            "Starting the Cadence OpenBench service",
        )
        if nginx_site:
            run_checked(["systemctl", "reload", "nginx"], "Reloading nginx")

    print("\nOpenBench server prepared.")
    print(f"Official checkout: {checkout}")
    print(f"Service unit:      {unit}")
    print(f"State directory:   {state_directory}")
    if nginx_site:
        print(f"nginx site:        {nginx_site}")
    if backup:
        print(f"Pre-migration backup: {backup}")
    if not args.activate:
        print("Services were not started and the database was not changed. Re-run with --activate during cutover.")


def main() -> int:
    try:
        configure(parser().parse_args())
        return 0
    except SetupError as error:
        print(f"setup-server: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Configure and launch a Cadence OpenBench worker on Linux, macOS, or Windows.

The setup path uses only the Python standard library. It creates a virtual
environment for the OpenBench client, installs the client's Python
dependencies, stores credentials outside version control, verifies the server,
and installs the native per-user launcher for the host platform.

The ``run`` command is an implementation detail used by the generated service.
It keeps secrets out of service definitions and command lines.
"""

from __future__ import annotations

import argparse
import getpass
import hashlib
import json
import os
import platform
import plistlib
import re
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path
from typing import NoReturn


SERVICE_NAME = "cadence-openbench-worker"
LAUNCHD_LABEL = "com.cadence.openbench-worker"
SCRIPT = Path(__file__).resolve()
PINS_PATH = SCRIPT.with_name("pins.json")

ENGINE_CREDENTIAL = "credentials.cadence"
FASTCHESS_METADATA = "fastchess-build.json"
WINDOWS_ALREADY_RUNNING = 75


class SetupError(RuntimeError):
    """A configuration error that should be shown without a traceback."""


def load_pins() -> dict[str, object]:
    try:
        value = json.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SetupError(f"cannot read reviewed pins from {PINS_PATH}: {error}") from error
    if not isinstance(value, dict) or value.get("schema") != 1:
        raise SetupError(f"{PINS_PATH} is not a supported pin manifest")
    for name in ("server", "client", "fastchess"):
        item = value.get(name)
        if not isinstance(item, dict):
            raise SetupError(f"{PINS_PATH} lacks the {name} pin")
        commit = item.get("commit")
        if not isinstance(commit, str) or re.fullmatch(r"[0-9a-f]{40}", commit) is None:
            raise SetupError(f"the {name} pin is not a full lowercase commit ID")
    return value


def pin(name: str) -> dict[str, object]:
    value = load_pins().get(name)
    if not isinstance(value, dict):
        raise SetupError(f"{PINS_PATH} lacks the {name} pin")
    return value


def host_system() -> str:
    name = platform.system()
    if name not in {"Linux", "Darwin", "Windows"}:
        raise SetupError(f"unsupported operating system: {name}")
    return name


def config_directory(system: str) -> Path:
    if system == "Windows":
        local = os.environ.get("LOCALAPPDATA")
        if not local:
            raise SetupError("LOCALAPPDATA is not set; cannot locate worker configuration")
        return Path(local) / "Cadence" / "OpenBench" / "config"

    if system == "Darwin":
        return Path.home() / "Library" / "Application Support" / "Cadence" / "OpenBench" / "config"

    configured = os.environ.get("XDG_CONFIG_HOME")
    base = Path(configured).expanduser() if configured else Path.home() / ".config"
    return base / "cadence-openbench"


def data_directory(system: str) -> Path:
    if system == "Windows":
        local = os.environ.get("LOCALAPPDATA")
        if not local:
            raise SetupError("LOCALAPPDATA is not set; cannot locate worker data")
        return Path(local) / "Cadence" / "OpenBench" / "data"
    if system == "Darwin":
        return Path.home() / "Library" / "Application Support" / "Cadence" / "OpenBench" / "data"
    configured = os.environ.get("XDG_DATA_HOME")
    base = Path(configured).expanduser() if configured else Path.home() / ".local" / "share"
    return base / "cadence-openbench"


def default_config_path(system: str) -> Path:
    return config_directory(system) / "worker.json"


def pinned_checkout(system: str) -> Path:
    commit = str(pin("client")["commit"])
    return data_directory(system) / "checkouts" / f"OpenBench-{commit}"


def pinned_venv(system: str) -> Path:
    commit = str(pin("client")["commit"])
    return data_directory(system) / "venvs" / f"client-{commit}"


def venv_python(system: str, config: dict[str, object] | None = None) -> Path:
    venv = Path(str(config["venv"])) if config else pinned_venv(system)
    if system == "Windows":
        return venv / "Scripts" / "python.exe"
    return venv / "bin" / "python"


def positive(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least one")
    return parsed


def setup_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Install the pinned official OpenBench client as a Cadence worker."
    )
    parser.add_argument("--server", help="OpenBench server URL; or OPENBENCH_SERVER")
    parser.add_argument("--username", help="OpenBench username; or OPENBENCH_USERNAME")
    parser.add_argument(
        "--threads",
        type=positive,
        help="measured concurrent games; required unless already configured",
    )
    parser.add_argument("--sockets", type=positive)
    parser.add_argument("--identity", help="worker name shown by OpenBench")
    parser.add_argument("--syzygy", help="optional Syzygy tablebase directory")
    parser.add_argument(
        "--build-jobs",
        type=positive,
        default=None,
        help="maximum concurrent compiler jobs used by fastchess and Cargo",
    )
    parser.add_argument(
        "--all-engines",
        action="store_true",
        help="do not prefer Cadence when the server offers multiple engines",
    )
    parser.add_argument(
        "--noisy",
        action="store_true",
        help="reject time-controlled work on a machine with unstable load",
    )
    parser.add_argument(
        "--without-engine-token",
        action="store_true",
        help="continue without the read-only Cadence GitHub token",
    )
    parser.add_argument(
        "--config",
        type=Path,
        help="configuration path; defaults to the platform user config directory",
    )
    parser.add_argument(
        "--no-service",
        action="store_true",
        help="configure the worker but do not install an automatic launcher",
    )
    parser.add_argument(
        "--no-start",
        action="store_true",
        help="install the automatic launcher without starting the worker now",
    )
    parser.add_argument(
        "--skip-server-check",
        action="store_true",
        help="do not verify the server URL and OpenBench credentials",
    )
    parser.add_argument(
        "--non-interactive",
        action="store_true",
        help="never prompt; missing secrets are errors",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="show the resolved plan without writing, downloading, or starting anything",
    )
    return parser


def run_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run a configured OpenBench worker.")
    parser.add_argument("run", help=argparse.SUPPRESS)
    parser.add_argument("--config", type=Path, required=True)
    return parser


def read_json(path: Path) -> dict[str, object]:
    if not path.is_file():
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SetupError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise SetupError(f"{path} must contain a JSON object")
    return value


def prompt_text(label: str, default: str | None, non_interactive: bool) -> str:
    if default:
        return default
    if non_interactive:
        raise SetupError(f"{label} is required in non-interactive mode")
    value = input(f"{label}: ").strip()
    if not value:
        raise SetupError(f"{label} cannot be empty")
    return value


def prompt_secret(
    label: str,
    environment_name: str,
    existing: str | None,
    non_interactive: bool,
) -> str:
    value = os.environ.get(environment_name) or existing
    if value:
        return value
    if non_interactive:
        raise SetupError(
            f"{label} is required; provide it through {environment_name}"
        )
    value = getpass.getpass(f"{label}: ").strip()
    if not value:
        raise SetupError(f"{label} cannot be empty")
    return value


def normalize_server(value: str) -> str:
    parsed = urllib.parse.urlparse(value)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        raise SetupError("server must be an absolute http:// or https:// URL")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise SetupError("server URL must not contain credentials, a query, or a fragment")
    return value.rstrip("/") + "/"


def resolve_configuration(
    args: argparse.Namespace, system: str, path: Path
) -> tuple[dict[str, object], str | None]:
    old = read_json(path)
    server = normalize_server(
        prompt_text(
            "OpenBench server URL",
            args.server or os.environ.get("OPENBENCH_SERVER") or old.get("server"),
            args.non_interactive,
        )
    )
    username = prompt_text(
        "OpenBench username",
        args.username or os.environ.get("OPENBENCH_USERNAME") or old.get("username"),
        args.non_interactive,
    )
    password = prompt_secret(
        "OpenBench password",
        "OPENBENCH_PASSWORD",
        old.get("password") if isinstance(old.get("password"), str) else None,
        args.non_interactive,
    )

    previous_threads = old.get("threads")
    threads = args.threads or (previous_threads if isinstance(previous_threads, int) else None)
    if threads is None:
        raise SetupError(
            "--threads is required for a new worker; measure concurrent copies of "
            "one Cadence bench binary and use the last value before their nps spread jumps"
        )
    identity = args.identity or old.get("identity") or socket.gethostname().split(".")[0]
    focus = [] if args.all_engines else ["Cadence"]
    checkout = pinned_checkout(system)
    venv = pinned_venv(system)
    runtime = data_directory(system) / "launcher"
    sockets = args.sockets or (
        old.get("sockets") if isinstance(old.get("sockets"), int) else 1
    )
    build_jobs = args.build_jobs or (
        old.get("build_jobs") if isinstance(old.get("build_jobs"), int) else 2
    )

    config: dict[str, object] = {
        "schema": 3,
        "repository": str(checkout),
        "client": str(checkout / "Client"),
        "venv": str(venv),
        "launcher_script": str(runtime / "setup-worker.py"),
        "pins_path": str(runtime / "pins.json"),
        "server": server,
        "username": username,
        "password": password,
        "threads": threads,
        "sockets": sockets,
        "identity": str(identity),
        "build_jobs": build_jobs,
        "focus": focus,
        "noisy": bool(args.noisy),
        "syzygy": args.syzygy,
    }

    token_path = path.parent / ENGINE_CREDENTIAL
    token = None if args.without_engine_token else os.environ.get("CADENCE_GITHUB_TOKEN")
    if not args.without_engine_token and not token and not token_path.is_file():
        token = prompt_secret(
            "GitHub token with read access to Cadence",
            "CADENCE_GITHUB_TOKEN",
            None,
            args.non_interactive,
        )

    config["engine_token_path"] = None if args.without_engine_token else str(token_path)
    config["fastchess_metadata_path"] = str(path.parent / FASTCHESS_METADATA)
    config["path_additions"] = platform_path_additions(system)
    return config, token


def secure_write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".new")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as output:
            output.write(content)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    os.replace(temporary, path)
    if os.name != "nt":
        path.chmod(stat.S_IRUSR | stat.S_IWUSR)


def write_configuration(path: Path, config: dict[str, object]) -> None:
    secure_write(path, json.dumps(config, indent=2, sort_keys=True) + "\n")


def check_prerequisites(system: str) -> None:
    missing: list[str] = []
    if shutil.which("git") is None:
        missing.append("git")
    if shutil.which("make") is None:
        missing.append("make")
    if shutil.which("cargo") is None:
        missing.append("cargo (installed through rustup)")
    if not (shutil.which("g++") or shutil.which("clang++")):
        missing.append("g++ or clang++")
    if not missing:
        return

    guidance = {
        "Linux": (
            "Debian/Ubuntu:\n"
            "  sudo apt-get update\n"
            "  sudo apt-get install -y --no-install-recommends "
            "ca-certificates curl git build-essential python3 python3-venv\n"
            "  if ! command -v rustup >/dev/null 2>&1; then\n"
            "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |\n"
            "      sh -s -- -y --profile minimal\n"
            "  fi\n"
            "  . \"$HOME/.cargo/env\""
        ),
        "Darwin": "Run xcode-select --install, then install Rust with rustup.",
        "Windows": (
            "Install Rust with rustup and MSYS2 with its make, MinGW g++, and coreutils; "
            "put the MSYS2 usr/bin and mingw64/bin directories on PATH."
        ),
    }[system]
    raise SetupError("missing prerequisites: " + ", ".join(missing) + ". " + guidance)


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


def required_file_sha256(path: Path, expected: object) -> None:
    actual = file_sha256(path)
    if not isinstance(expected, str) or actual != expected:
        raise SetupError(
            f"{path} does not match the reviewed dependency declaration: "
            f"expected {expected}, found {actual}"
        )


def verify_checkout(checkout: Path) -> None:
    expected = str(pin("client")["commit"])
    if not (checkout / ".git").is_dir():
        raise SetupError(f"{checkout} is not an OpenBench Git checkout")
    actual = run_output(["git", "rev-parse", "HEAD"], "Reading OpenBench commit", cwd=checkout)
    if actual != expected:
        raise SetupError(f"OpenBench checkout is {actual}, expected {expected}: {checkout}")
    changed = run_output(
        ["git", "status", "--porcelain", "--untracked-files=no"],
        "Checking the official client worktree",
        cwd=checkout,
    )
    if changed:
        raise SetupError(f"tracked files changed in the official client checkout:\n{changed}")
    if not (checkout / "Client" / "client.py").is_file():
        raise SetupError(f"the pinned checkout lacks Client/client.py: {checkout}")
    required_file_sha256(
        checkout / "Client" / "requirements.txt",
        pin("client").get("requirements_sha256"),
    )


def install_official_client(system: str) -> Path:
    checkout = pinned_checkout(system)
    if checkout.exists():
        verify_checkout(checkout)
        return checkout

    checkout.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=".OpenBench-new-", dir=checkout.parent))
    client_pin = pin("client")
    repository = str(client_pin["repository"])
    commit = str(client_pin["commit"])
    try:
        run_checked(["git", "init", str(temporary)], "Creating the OpenBench checkout")
        run_checked(
            ["git", "remote", "add", "origin", repository],
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
            "Checking out the reviewed OpenBench commit",
            cwd=temporary,
        )
        verify_checkout(temporary)
        os.replace(temporary, checkout)
    except Exception:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return checkout


def install_runtime_files(system: str) -> Path:
    runtime = data_directory(system) / "launcher"
    runtime.mkdir(parents=True, exist_ok=True)
    installed_script = runtime / "setup-worker.py"
    installed_pins = runtime / "pins.json"
    if SCRIPT != installed_script:
        shutil.copy2(SCRIPT, installed_script)
    if PINS_PATH != installed_pins:
        shutil.copy2(PINS_PATH, installed_pins)
    if system != "Windows":
        installed_script.chmod(installed_script.stat().st_mode | stat.S_IXUSR)
    return installed_script


def install_python_dependencies(system: str, config: dict[str, object]) -> None:
    python = venv_python(system, config)
    if not python.is_file():
        run_checked(
            [sys.executable, "-m", "venv", str(config["venv"])],
            "Creating the versioned client virtual environment",
        )
    requirements = Path(str(config["client"])) / "requirements.txt"
    commit = str(pin("client")["commit"])
    lock = config_directory(system) / f"client-python-packages-{commit}.txt"
    install_arguments = ["-r", str(lock)] if lock.is_file() else ["-r", str(requirements)]
    run_checked(
        [str(python), "-m", "pip", "install", *install_arguments],
        "Installing OpenBench Python dependencies",
    )
    if not lock.is_file():
        resolved = run_output(
            [str(python), "-m", "pip", "freeze", "--all"],
            "Recording resolved OpenBench Python dependencies",
        )
        secure_write(lock, resolved + "\n")


def server_json(config: dict[str, object], endpoint_name: str) -> dict[str, object]:
    endpoint = urllib.parse.urljoin(str(config["server"]), endpoint_name + "/")
    fields = urllib.parse.urlencode(
        {"username": config["username"], "password": config["password"]}
    ).encode("utf-8")
    request = urllib.request.Request(endpoint, data=fields, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            payload = json.load(response)
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
        raise SetupError(f"cannot read {endpoint}: {error}") from error
    if not isinstance(payload, dict):
        raise SetupError(f"{endpoint} did not return a JSON object")
    if "error" in payload:
        raise SetupError(f"{endpoint} returned: {payload['error']}")
    return payload


def verify_client_pin(config: dict[str, object]) -> None:
    payload = server_json(config, "clientVersionRef")
    reviewed = pin("client")
    actual = (
        payload.get("client_repo_url"),
        payload.get("client_repo_ref"),
        payload.get("client_version"),
    )
    expected = (
        reviewed.get("repository"),
        reviewed.get("commit"),
        reviewed.get("version"),
    )
    if actual != expected:
        raise SetupError(
            "the server does not name the reviewed OpenBench client pin: "
            f"expected {expected!r}, received {actual!r}"
        )


def verify_server(config: dict[str, object]) -> None:
    verify_client_pin(config)


def systemd_quote(value: Path) -> str:
    escaped = (
        str(value)
        .replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("%", "%%")
    )
    return '"' + escaped + '"'


def install_linux_service(
    config_path: Path, config: dict[str, object], start: bool
) -> tuple[Path, str]:
    systemctl = shutil.which("systemctl")
    if not systemctl:
        raise SetupError("systemctl is unavailable; use --no-service and run the printed command")
    unit_path = Path.home() / ".config" / "systemd" / "user" / f"{SERVICE_NAME}.service"
    unit = f"""[Unit]
Description=OpenBench worker for Cadence
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory={systemd_quote(Path(str(config["client"])))}
ExecStart={systemd_quote(venv_python("Linux", config))} -u {systemd_quote(Path(str(config["launcher_script"])))} run --config {systemd_quote(config_path)}
Restart=always
RestartSec=15
KillSignal=SIGTERM
TimeoutStopSec=120

[Install]
WantedBy=default.target
"""
    unit_path.parent.mkdir(parents=True, exist_ok=True)
    unit_path.write_text(unit, encoding="utf-8")
    run_checked([systemctl, "--user", "daemon-reload"], "Reloading the user systemd manager")
    command = [systemctl, "--user", "enable"]
    command.append(f"{SERVICE_NAME}.service")
    run_checked(command, "Enabling the OpenBench worker service")
    loginctl = shutil.which("loginctl")
    if loginctl:
        linger = subprocess.run(
            [loginctl, "show-user", str(os.getuid()), "--property=Linger", "--value"],
            check=False,
            capture_output=True,
            text=True,
        ).stdout.strip()
        if linger != "yes":
            print(
                "Warning: systemd user lingering is disabled. This worker starts at login, "
                f"not boot; for a headless host run: sudo loginctl enable-linger {getpass.getuser()}"
            )
    if start:
        run_checked(
            [systemctl, "--user", "restart", f"{SERVICE_NAME}.service"],
            "Starting the OpenBench worker service",
        )
    return unit_path, f"journalctl --user -u {SERVICE_NAME} -f"


def install_macos_service(
    config_path: Path, config: dict[str, object], start: bool
) -> tuple[Path, str]:
    launch_agents = Path.home() / "Library" / "LaunchAgents"
    log_path = Path.home() / "Library" / "Logs" / "openbench-worker.log"
    plist_path = launch_agents / f"{LAUNCHD_LABEL}.plist"
    launch_agents.mkdir(parents=True, exist_ok=True)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    document = {
        "Label": LAUNCHD_LABEL,
        "ProgramArguments": [
            str(venv_python("Darwin", config)),
            "-u",
            str(config["launcher_script"]),
            "run",
            "--config",
            str(config_path),
        ],
        "WorkingDirectory": str(config["client"]),
        "RunAtLoad": True,
        "KeepAlive": True,
        "ThrottleInterval": 15,
        "ProcessType": "Standard",
        "StandardOutPath": str(log_path),
        "StandardErrorPath": str(log_path),
    }
    with plist_path.open("wb") as output:
        plistlib.dump(document, output, sort_keys=False)
    if start:
        domain = f"gui/{os.getuid()}"
        subprocess.run(
            ["launchctl", "bootout", domain, str(plist_path)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        run_checked(
            ["launchctl", "bootstrap", domain, str(plist_path)],
            "Starting the launchd worker",
        )
    return plist_path, f"tail -f {log_path}"


def platform_path_additions(system: str) -> list[str]:
    candidates = [Path.home() / ".cargo" / "bin"]
    if system == "Darwin":
        candidates.extend([Path("/opt/homebrew/bin"), Path("/usr/local/bin")])
    elif system == "Windows":
        candidates.extend(
            [Path("C:/msys64/mingw64/bin"), Path("C:/msys64/usr/bin")]
        )
    return [str(path) for path in candidates if path.is_dir()]


def worker_environment(config: dict[str, object]) -> dict[str, str]:
    environment = os.environ.copy()
    environment.pop("CADENCE_GITHUB_TOKEN", None)
    environment["OPENBENCH_SERVER"] = str(config["server"])
    environment["OPENBENCH_USERNAME"] = str(config["username"])
    environment["OPENBENCH_PASSWORD"] = str(config["password"])
    environment["CARGO_BUILD_JOBS"] = str(config.get("build_jobs", 2))
    additions = config.get("path_additions", [])
    if isinstance(additions, list):
        environment["PATH"] = os.pathsep.join(
            [str(value) for value in additions] + [environment.get("PATH", "")]
        )
    return environment


def restore_engine_credential(config: dict[str, object], client: Path) -> None:
    configured = config.get("engine_token_path")
    destination = client / ENGINE_CREDENTIAL
    if configured is None:
        destination.unlink(missing_ok=True)
        return
    source = Path(str(configured)).expanduser().resolve()
    try:
        token = source.read_text(encoding="utf-8").splitlines()[0].strip()
    except (OSError, IndexError) as error:
        raise SetupError(f"cannot restore the Cadence token from {source}: {error}") from error
    if not token:
        raise SetupError(f"the Cadence token at {source} is empty")
    secure_write(destination, token + "\n")


def version_tuple(value: str) -> tuple[int, ...]:
    try:
        return tuple(int(part) for part in value.split("."))
    except ValueError as error:
        raise SetupError(f"invalid version from OpenBench: {value!r}") from error


def program_version(path: Path) -> str | None:
    if not path.is_file():
        return None
    for option in ("--version", "version", "-v", "-version"):
        try:
            result = subprocess.run(
                [str(path), option],
                check=False,
                capture_output=True,
                text=True,
                timeout=10,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue
        match = re.search(r"\d+\.\d+(?:\.\d+)?", result.stdout + result.stderr)
        if match:
            return match.group(0)
    return None


def fastchess_binary(system: str, client: Path) -> Path:
    suffix = ".exe" if system == "Windows" else ""
    return client / f"fastchess-ob{suffix}"


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


def fastchess_server_configuration(config: dict[str, object]) -> dict[str, str]:
    payload = server_json(config, "clientMatchRunnerVersionRef")
    fields = {
        "repository": payload.get("fastchess_repo_url"),
        "ref": payload.get("fastchess_repo_ref"),
        "minimum_version": payload.get("fastchess_min_version"),
    }
    if not all(isinstance(value, str) and value for value in fields.values()):
        raise SetupError("the server returned an incomplete fastchess configuration")
    if re.fullmatch(r"[0-9a-fA-F]{40}", fields["ref"]) is None:
        raise SetupError(
            "the server's fastchess ref is not an immutable full commit: "
            f"{fields['ref']!r}"
        )
    version_tuple(fields["minimum_version"])
    reviewed = pin("fastchess")
    expected = {
        "repository": reviewed.get("repository"),
        "ref": reviewed.get("commit"),
        "minimum_version": reviewed.get("minimum_version"),
    }
    if fields != expected:
        raise SetupError(
            "the server does not name the reviewed fastchess pin: "
            f"expected {expected!r}, received {fields!r}"
        )
    return {key: str(value) for key, value in fields.items()}


def extract_zip_safely(archive_path: Path, destination: Path) -> Path:
    with zipfile.ZipFile(archive_path) as archive:
        root = destination.resolve()
        for member in archive.infolist():
            target = (destination / member.filename).resolve()
            if not target.is_relative_to(root):
                raise SetupError(f"unsafe path in the fastchess archive: {member.filename}")
        archive.extractall(destination)
    roots = [entry for entry in destination.iterdir() if entry.is_dir()]
    if len(roots) != 1:
        raise SetupError("the fastchess archive did not contain one source directory")
    return roots[0]


def build_fastchess(
    system: str,
    client: Path,
    desired: dict[str, str],
    build_jobs: int,
    environment: dict[str, str],
) -> tuple[Path, str]:
    search_path = environment.get("PATH")
    compiler = shutil.which("g++", path=search_path) or shutil.which(
        "clang++", path=search_path
    )
    make = shutil.which("make", path=search_path)
    if not compiler or not make:
        raise SetupError("building fastchess needs make and g++ or clang++ on PATH")

    archive_url = (
        desired["repository"].rstrip("/")
        + "/archive/"
        + urllib.parse.quote(desired["ref"], safe="")
        + ".zip"
    )
    print(f"Downloading pinned fastchess {desired['ref']} from {desired['repository']}...")
    try:
        with urllib.request.urlopen(archive_url, timeout=60) as response:
            archive_bytes = response.read()
    except (OSError, urllib.error.URLError) as error:
        raise SetupError(f"cannot download {archive_url}: {error}") from error

    with tempfile.TemporaryDirectory(prefix="cadence-fastchess-") as temporary:
        temporary_path = Path(temporary)
        archive_path = temporary_path / "fastchess.zip"
        archive_path.write_bytes(archive_bytes)
        source = extract_zip_safely(archive_path, temporary_path / "source")
        run_checked(
            [make, f"-j{build_jobs}", f"CXX={compiler}"],
            f"Building fastchess with at most {build_jobs} compiler jobs",
            cwd=source,
            environment=environment,
        )
        built_name = "fastchess.exe" if system == "Windows" else "fastchess"
        built = source / built_name
        reported_version = program_version(built)
        if reported_version is None:
            raise SetupError(f"the built fastchess binary at {built} reports no version")
        if version_tuple(reported_version) < version_tuple(desired["minimum_version"]):
            raise SetupError(
                f"fastchess {reported_version} is below the server minimum "
                f"{desired['minimum_version']}"
            )
        expected_version = str(pin("fastchess")["reported_version"])
        if reported_version != expected_version:
            raise SetupError(
                f"pinned fastchess reports {reported_version}, reviewed source reports "
                f"{expected_version}"
            )

        target = fastchess_binary(system, client)
        pending = target.with_name(target.name + ".new")
        shutil.copy2(built, pending)
        if system != "Windows":
            pending.chmod(pending.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        os.replace(pending, target)
        return target, reported_version


def reconcile_fastchess(
    system: str,
    config_path: Path,
    config: dict[str, object],
    client: Path,
    environment: dict[str, str],
) -> None:
    desired = fastchess_server_configuration(config)
    metadata_value = config.get("fastchess_metadata_path")
    metadata_path = (
        Path(str(metadata_value)).expanduser().resolve()
        if metadata_value
        else config_path.parent / FASTCHESS_METADATA
    )
    metadata = read_json(metadata_path)
    binary = fastchess_binary(system, client)
    reported_version = program_version(binary)
    binary_sha256 = file_sha256(binary)
    pinned = all(metadata.get(key) == value for key, value in desired.items()) and (
        binary_sha256 is not None and metadata.get("binary_sha256") == binary_sha256
    )
    acceptable = (
        reported_version is not None
        and reported_version == str(pin("fastchess")["reported_version"])
        and version_tuple(reported_version) >= version_tuple(desired["minimum_version"])
    )
    if pinned and acceptable:
        print(
            f"Using pinned fastchess {desired['ref']} "
            f"(reported version {reported_version})."
        )
        return

    binary, reported_version = build_fastchess(
        system,
        client,
        desired,
        int(config.get("build_jobs", 2)),
        environment,
    )
    secure_write(
        metadata_path,
        json.dumps(
            {
                **desired,
                "binary": str(binary),
                "binary_sha256": file_sha256(binary),
                "reported_version": reported_version,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
    )


def windows_command(path: Path) -> str:
    return '"' + str(path).replace('"', '""') + '"'


def acquire_windows_worker_mutex(config_path: Path) -> int | None:
    """Own one Windows worker per configuration, without a stale PID file."""
    import ctypes
    from ctypes import wintypes

    name_hash = hashlib.sha256(str(config_path).encode("utf-8")).hexdigest()
    name = f"Local\\CadenceOpenBenchWorker-{name_hash}"
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    create_mutex = kernel32.CreateMutexW
    create_mutex.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    create_mutex.restype = wintypes.HANDLE
    handle = create_mutex(None, False, name)
    error = ctypes.get_last_error()
    if not handle:
        raise SetupError(f"cannot create the Windows worker mutex: error {error}")
    if error == 183:  # ERROR_ALREADY_EXISTS
        close_windows_handle(int(handle))
        return None
    return int(handle)


def close_windows_handle(handle: int) -> None:
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    close_handle = kernel32.CloseHandle
    close_handle.argtypes = [wintypes.HANDLE]
    close_handle.restype = wintypes.BOOL
    close_handle(wintypes.HANDLE(handle))


def install_windows_launcher(
    config_path: Path, config: dict[str, object], start: bool
) -> tuple[Path, str]:
    directory = config_directory("Windows")
    directory.mkdir(parents=True, exist_ok=True)
    log_path = directory / "worker.log"
    runner = directory / "run-worker.cmd"
    runner_text = f"""@echo off
setlocal
:restart
{windows_command(venv_python("Windows", config))} -u {windows_command(Path(str(config["launcher_script"])))} run --config {windows_command(config_path)} >> {windows_command(log_path)} 2>&1
if %errorlevel% equ {WINDOWS_ALREADY_RUNNING} exit /b 0
timeout /t 15 /nobreak >nul
goto restart
"""
    runner.write_text(runner_text, encoding="utf-8", newline="\r\n")

    roaming = os.environ.get("APPDATA")
    if not roaming:
        raise SetupError("APPDATA is not set; cannot locate the Windows Startup directory")
    startup = Path(roaming) / "Microsoft" / "Windows" / "Start Menu" / "Programs" / "Startup"
    startup.mkdir(parents=True, exist_ok=True)
    entry = startup / "Cadence OpenBench Worker.cmd"
    entry.write_text(
        f"@echo off\nstart \"Cadence OpenBench Worker\" /min cmd /c call {windows_command(runner)}\n",
        encoding="utf-8",
        newline="\r\n",
    )
    if start:
        # Wake a stopped installation as well as reconfigure a running one.
        # The official client consumes this as a graceful stop request; the
        # named mutex keeps the newly launched wrapper from starting a second
        # client while the existing wrapper restarts with the new config.
        (Path(str(config["client"])) / "openbench.exit").touch()
        os.startfile(entry)  # type: ignore[attr-defined]
    return entry, f"Get-Content -Wait {windows_command(log_path)}"


def install_service(
    system: str, config_path: Path, config: dict[str, object], start: bool
) -> tuple[Path, str]:
    if system == "Linux":
        return install_linux_service(config_path, config, start)
    if system == "Darwin":
        return install_macos_service(config_path, config, start)
    return install_windows_launcher(config_path, config, start)


def worker_arguments(config: dict[str, object]) -> list[str]:
    arguments = [
        "-T",
        str(config["threads"]),
        "-N",
        str(config["sockets"]),
        "-I",
        str(config["identity"]),
    ]
    syzygy = config.get("syzygy")
    if syzygy:
        arguments.extend(["--syzygy", str(syzygy)])
    if config.get("noisy"):
        arguments.append("--noisy")
    focus = config.get("focus")
    if isinstance(focus, list) and focus:
        arguments.append("--focus")
        arguments.extend(str(engine) for engine in focus)
    return arguments


def run_worker_once(config_path: Path) -> NoReturn:
    config = read_json(config_path)
    required = {
        "repository",
        "client",
        "venv",
        "launcher_script",
        "server",
        "username",
        "password",
        "threads",
        "sockets",
        "identity",
        "engine_token_path",
        "fastchess_metadata_path",
    }
    missing = sorted(required.difference(config))
    if missing:
        raise SetupError(f"{config_path} lacks: {', '.join(missing)}")

    repository = Path(str(config["repository"])).resolve()
    verify_checkout(repository)
    client = Path(str(config["client"])).resolve()
    if client != repository / "Client":
        raise SetupError(f"configured Client path is outside the reviewed checkout: {client}")
    client_script = client / "client.py"
    if not client_script.is_file():
        raise SetupError(f"OpenBench client not found at {client_script}")

    system = host_system()
    environment = worker_environment(config)
    verify_client_pin(config)
    restore_engine_credential(config, client)
    reconcile_fastchess(system, config_path, config, client, environment)

    command = [
        sys.executable,
        "-u",
        str(client_script),
        "--no-client-downloads",
    ] + worker_arguments(config)
    os.chdir(client)
    if os.name == "nt":
        raise SystemExit(subprocess.call(command, env=environment))
    os.execve(sys.executable, command, environment)
    raise AssertionError("os.execve returned")


def run_worker(config_path: Path) -> NoReturn:
    if os.name != "nt":
        run_worker_once(config_path)

    mutex = acquire_windows_worker_mutex(config_path)
    if mutex is None:
        raise SystemExit(WINDOWS_ALREADY_RUNNING)
    try:
        run_worker_once(config_path)
    finally:
        close_windows_handle(mutex)
    raise AssertionError("run_worker_once returned")


def show_plan(
    system: str, path: Path, config: dict[str, object], token: str | None, args: argparse.Namespace
) -> None:
    print("Dry run; no files, packages, services, or network connections were changed.")
    print(f"Platform:       {system}")
    print(f"Official repo:  {config['repository']}")
    print(f"Official ref:   {pin('client')['commit']}")
    print(f"Configuration:  {path}")
    print(f"Server:         {config['server']}")
    print(f"Identity:       {config['identity']}")
    print(f"Game threads:   {config['threads']}")
    print(f"Compiler jobs:  {config['build_jobs']}")
    print(f"Cadence token:  {'new token supplied' if token else 'existing file or omitted'}")
    print(f"Service:        {'not installed' if args.no_service else 'platform user launcher'}")
    print(f"Start now:      {not args.no_start and not args.no_service}")


def configure_worker(args: argparse.Namespace) -> None:
    system = host_system()
    path = (args.config or default_config_path(system)).expanduser().resolve()
    config, token = resolve_configuration(args, system, path)
    check_prerequisites(system)

    if args.dry_run:
        show_plan(system, path, config, token, args)
        return

    checkout = install_official_client(system)
    if checkout != Path(str(config["repository"])):
        raise SetupError("the installed OpenBench checkout differs from the resolved plan")
    config["launcher_script"] = str(install_runtime_files(system))
    install_python_dependencies(system, config)
    if not args.skip_server_check:
        if str(config["server"]).startswith("http://"):
            print(
                "Warning: this server uses unencrypted HTTP. Use it only on a "
                "trusted LAN or VPN; use HTTPS for remote testers."
            )
        print("Verifying the OpenBench account and server...")
        verify_server(config)

    write_configuration(path, config)
    if token:
        secure_write(
            Path(str(config["engine_token_path"])),
            token.rstrip("\r\n") + "\n",
        )
    elif not args.without_engine_token and not Path(
        str(config["engine_token_path"])
    ).is_file():
        raise SetupError("Cadence is private in this OpenBench configuration and needs a GitHub token")

    client = Path(str(config["client"]))
    restore_engine_credential(config, client)
    if not args.skip_server_check:
        environment = worker_environment(config)
        reconcile_fastchess(system, path, config, client, environment)

    launcher: Path | None = None
    log_command: str | None = None
    if not args.no_service:
        launcher, log_command = install_service(
            system, path, config, not args.no_start
        )

    print("\nWorker configured.")
    print(f"Configuration: {path}")
    if launcher:
        print(f"Launcher:      {launcher}")
    else:
        print(
            "Run manually:  "
            + f"{venv_python(system, config)} -u {config['launcher_script']} run --config {path}"
        )
    if log_command:
        print(f"Logs:          {log_command}")
    print(
        "Concurrency came from --threads. Keep it only if concurrent copies of one "
        "Cadence bench binary remain close in nps under sustained load."
    )
    if system == "Windows":
        print(
            "Windows onboarding is configured, but Cadence workloads remain disabled "
            "for Windows until the engine build is verified and Engines/Cadence.json "
            "advertises that platform."
        )


def main() -> int:
    try:
        if len(sys.argv) > 1 and sys.argv[1] == "run":
            args = run_parser().parse_args()
            run_worker(args.config.expanduser().resolve())
        else:
            configure_worker(setup_parser().parse_args())
        return 0
    except SetupError as error:
        print(f"setup-worker: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

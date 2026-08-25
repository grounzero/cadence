# SPDX-License-Identifier: GPL-3.0-or-later
"""Static and mocked checks for the OpenBench deployment layer."""

from __future__ import annotations

import importlib.util
import json
import os
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]


def load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {filename}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PinTests(unittest.TestCase):
    def test_every_source_pin_is_a_full_commit(self) -> None:
        pins = json.loads((ROOT / "pins.json").read_text(encoding="utf-8"))
        for name in ("server", "client", "fastchess"):
            with self.subTest(name=name):
                self.assertRegex(pins[name]["commit"], r"^[0-9a-f]{40}$")

    def test_server_config_is_generated_from_pin_manifest(self) -> None:
        server = load("cadence_setup_server", "setup-server.py")
        pins = json.loads((ROOT / "pins.json").read_text(encoding="utf-8"))
        config = json.loads(server.generated_openbench_config())
        self.assertEqual(config["client_repo_url"], pins["client"]["repository"])
        self.assertEqual(config["client_repo_ref"], pins["client"]["commit"])
        self.assertEqual(config["client_version"], pins["client"]["version"])
        self.assertEqual(config["fastchess_repo_url"], pins["fastchess"]["repository"])
        self.assertEqual(config["fastchess_repo_ref"], pins["fastchess"]["commit"])
        self.assertEqual(
            config["fastchess_min_version"], pins["fastchess"]["minimum_version"]
        )

    def test_service_template_resolves_every_placeholder(self) -> None:
        server = load("cadence_setup_server_template", "setup-server.py")
        rendered = server.render(
            ROOT / "deploy" / "cadence-openbench.service.in",
            {
                "SERVICE_USER": "openbench",
                "SERVICE_GROUP": "openbench",
                "STATE_DIRECTORY": "/var/lib/cadence-openbench",
                "CONFIG_DIRECTORY": "/etc/cadence-openbench",
                "CHECKOUT": "/opt/cadence-openbench/checkouts/example",
                "VENV": "/opt/cadence-openbench/venvs/example",
            },
        )
        self.assertIsNone(re.search(r"@@[A-Z_]+@@", rendered))

    def test_cutover_disables_an_inactive_enabled_previous_service(self) -> None:
        server = load("cadence_setup_server_cutover", "setup-server.py")
        with (
            mock.patch.object(server, "service_is_active", side_effect=[False, False]),
            mock.patch.object(server, "service_is_enabled", return_value=True),
            mock.patch.object(server, "run_checked") as run_checked,
        ):
            server.stop_for_activation("openbench.service")

        run_checked.assert_called_once_with(
            ["systemctl", "disable", "openbench.service"],
            "Disabling the previous OpenBench service openbench.service",
        )

    def test_nginx_preserves_the_outer_scheme_only_in_proxy_mode(self) -> None:
        server = load("cadence_setup_server_nginx", "setup-server.py")
        template = ROOT / "deploy" / "nginx-openbench.conf.in"
        common = {
            "LISTEN": "127.0.0.1:8080",
            "SERVER_NAMES": "openbench.example",
            "STATIC_DIRECTORY": "/var/lib/cadence-openbench/static",
        }
        for behind_proxy, expected in (
            (False, "$scheme"),
            (True, "$http_x_forwarded_proto"),
        ):
            with self.subTest(behind_proxy=behind_proxy):
                rendered = server.render(
                    template,
                    {
                        **common,
                        "FORWARDED_PROTO": server.forwarded_proto_source(behind_proxy),
                    },
                )
                self.assertIn(
                    f"proxy_set_header X-Forwarded-Proto {expected};", rendered
                )
                self.assertIsNone(re.search(r"@@[A-Z_]+@@", rendered))


class WorkerTests(unittest.TestCase):
    def test_windows_paths_are_outside_source_checkout(self) -> None:
        worker = load("cadence_setup_worker_windows", "setup-worker.py")
        with mock.patch.dict(os.environ, {"LOCALAPPDATA": r"C:\\Users\\tester\\AppData\\Local"}):
            config = worker.config_directory("Windows")
            data = worker.data_directory("Windows")
        self.assertIn("Cadence", str(config))
        self.assertIn("Cadence", str(data))
        self.assertNotEqual(config, data)

    def test_fastchess_configuration_requires_exact_reviewed_pin(self) -> None:
        worker = load("cadence_setup_worker_fastchess", "setup-worker.py")
        reviewed = worker.pin("fastchess")
        payload = {
            "fastchess_repo_url": reviewed["repository"],
            "fastchess_repo_ref": reviewed["commit"],
            "fastchess_min_version": reviewed["minimum_version"],
        }
        with mock.patch.object(worker, "server_json", return_value=payload):
            actual = worker.fastchess_server_configuration({})
        self.assertEqual(actual["ref"], reviewed["commit"])

        payload["fastchess_repo_ref"] = "master"
        with mock.patch.object(worker, "server_json", return_value=payload):
            with self.assertRaises(worker.SetupError):
                worker.fastchess_server_configuration({})

    def test_disabling_the_engine_token_removes_the_runtime_copy(self) -> None:
        worker = load("cadence_setup_worker_token", "setup-worker.py")
        with tempfile.TemporaryDirectory() as temporary_name:
            client = Path(temporary_name)
            runtime_token = client / worker.ENGINE_CREDENTIAL
            runtime_token.write_text("stale-token\n", encoding="utf-8")

            worker.restore_engine_credential({"engine_token_path": None}, client)

            self.assertFalse(runtime_token.exists())

    def test_existing_windows_launcher_is_started_when_requested(self) -> None:
        worker = load("cadence_setup_worker_start_windows", "setup-worker.py")
        with tempfile.TemporaryDirectory() as temporary_name:
            root = Path(temporary_name)
            config_directory = root / "config"
            client = root / "client"
            client.mkdir()
            config = {
                "client": str(client),
                "launcher_script": str(root / "setup-worker.py"),
                "venv": str(root / "venv"),
            }
            config_path = config_directory / "worker.json"
            environment = {"APPDATA": str(root / "roaming")}
            with (
                mock.patch.dict(os.environ, environment),
                mock.patch.object(
                    worker, "config_directory", return_value=config_directory
                ),
                mock.patch.object(worker.os, "startfile", create=True) as startfile,
            ):
                entry, _ = worker.install_windows_launcher(
                    config_path, config, start=False
                )
                worker.install_windows_launcher(config_path, config, start=True)

            startfile.assert_called_once_with(entry)
            self.assertTrue((client / "openbench.exit").is_file())
            runner = (config_directory / "run-worker.cmd").read_text(encoding="utf-8")
            self.assertIn(
                f"if %errorlevel% equ {worker.WINDOWS_ALREADY_RUNNING}", runner
            )

    def test_second_windows_worker_exits_at_the_mutex(self) -> None:
        worker = load("cadence_setup_worker_mutex", "setup-worker.py")
        config_path = Path("C:/Cadence/OpenBench/config/worker.json")
        with (
            mock.patch.object(worker.os, "name", "nt"),
            mock.patch.object(worker, "acquire_windows_worker_mutex", return_value=None),
            mock.patch.object(worker, "run_worker_once") as run_once,
        ):
            with self.assertRaises(SystemExit) as raised:
                worker.run_worker(config_path)

        self.assertEqual(raised.exception.code, worker.WINDOWS_ALREADY_RUNNING)
        run_once.assert_not_called()


class RestoreTests(unittest.TestCase):
    def test_restore_owns_the_state_directory_it_writes_into(self) -> None:
        restore = load("cadence_restore_server_owner", "restore-server.py")
        with tempfile.TemporaryDirectory() as temporary_name:
            state = Path(temporary_name) / "state"
            media = state / "media"
            database = state / "db.sqlite3"
            state.mkdir()
            media.mkdir()
            database.write_bytes(b"database")
            child = media / "game.pgn"
            child.write_text("game", encoding="utf-8")

            with mock.patch.object(restore.shutil, "chown") as chown:
                restore.set_restored_owner(
                    state, database, media, "openbench", "openbench"
                )

            owned = [call.args[0] for call in chown.call_args_list]
            self.assertEqual(owned, [state, database, media, child])


if __name__ == "__main__":
    unittest.main()

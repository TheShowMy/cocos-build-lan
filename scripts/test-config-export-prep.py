"""Validate the shipped preparation JSON without touching installed projects."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


PAYLOAD = json.loads(
    (Path(__file__).resolve().parents[1] / "examples/config-export-prep.json")
    .read_text(encoding="utf-8")
)


class PrepTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="配置准备 测试 ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for item in PAYLOAD["files"]:
            (self.root / item["path"]).write_text(item["content"], encoding="utf-8")
        self.modules = {}
        for name in ("models", "lib"):
            spec = importlib.util.spec_from_file_location(name, self.root / f"{name}.py")
            module = importlib.util.module_from_spec(spec)
            self.modules[name] = module
            with patch.dict(sys.modules, self.modules):
                spec.loader.exec_module(module)
        self.lib = self.modules["lib"]
        self.project = self.root / "中文 游戏 project"
        (self.project / "python/config").mkdir(parents=True)
        (self.project / "python/config/channel.json").write_text("{}", encoding="utf-8")
        self.person = self.modules["models"].Person(
            str(self.project), "japan_web", "release", "false", "test", "review", "pro"
        )

    def test_branch_rules(self):
        for mode, censor, expected in [
            ("release", "false", "pro"), ("test", "false", "test"),
            ("prod", "false", "test"), ("pro", "false", "test"),
            ("RELEASE", "false", "test"), ("release", "true", "review"),
            ("test", "true", "review"),
        ]:
            with self.subTest(mode=mode, censor=censor):
                self.person.build_mode = mode
                self.person.censor_switch = censor
                self.assertEqual(self.lib.select_branch(self.person), expected)
        for value in (None, ""):
            self.person.censor_branch = value
            self.assertEqual(self.lib.select_branch(self.person), "master")
            self.person.censor_switch = "false"
            self.person.test_branch = value
            self.assertEqual(self.lib.select_branch(self.person), "master")
            self.person.build_mode = "release"
            self.person.release_branch = value
            self.assertEqual(self.lib.select_branch(self.person), "master")
            self.person.censor_switch = "true"

    def test_node_preference_and_no_shell_wrapper(self):
        node = self.root / "Node 安装/node.exe"
        cli = node.parent / "node_modules/npm/bin/npm-cli.js"
        cli.parent.mkdir(parents=True)
        node.touch()
        cli.touch()
        with patch.dict(os.environ, {"COCOS_BUILD_LAN_NODE_EXE": str(node)}, clear=True):
            self.assertEqual(self.lib.npm_command(), [str(node.resolve()), str(cli.resolve())])
            cli.unlink()
            with patch.object(self.lib.shutil, "which", return_value=None):
                with self.assertRaisesRegex(RuntimeError, "npm-cli.js"):
                    self.lib.npm_command()

    def test_arguments_streams_and_data_locations(self):
        with patch.object(self.lib, "npm_command", return_value=["C:/node/node.exe", "npm-cli.js"]), \
                patch.object(self.lib.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            self.lib.process_person(self.person)
        args = run.call_args.args[0]
        self.assertIn("--package=@tsixi/export-config@0.1.2", args)
        self.assertIn("--registry=http://192.168.2.33:4873", args)
        self.assertIn("--force-export", args)
        self.assertNotIn("--force-clean", args)
        for option, expected in {
            "--project-root": str(self.project),
            "--channel-config": str(self.project / "python/config/channel.json"),
            "--runtime-dir": str(self.project / "tool_data/export_config"),
            "-p": str(self.root / "data/japan_web"), "-b": "pro",
        }.items():
            self.assertEqual(args[args.index(option) + 1], expected)
        kwargs = run.call_args.kwargs
        self.assertEqual(kwargs["cwd"], self.project)
        self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
        for key in ("stdout", "stderr", "shell", "start_new_session"):
            self.assertNotIn(key, kwargs)

    def test_inherited_npx_entry_is_not_used_as_npm(self):
        node = self.root / "Node 安装/node.exe"
        cli = node.parent / "node_modules/npm/bin/npm-cli.js"
        npx = cli.with_name("npx-cli.js")
        cli.parent.mkdir(parents=True)
        node.touch()
        cli.touch()
        npx.touch()
        with patch.dict(os.environ, {"COCOS_BUILD_LAN_NODE_EXE": str(node),
                                     "npm_execpath": str(npx)}, clear=True):
            self.assertEqual(self.lib.npm_command(), [str(node.resolve()), str(cli.resolve())])

    def test_error_output_is_utf8_even_with_windows_legacy_encoding(self):
        env = dict(os.environ, PYTHONIOENCODING="gbk")
        result = subprocess.run([
            sys.executable, str(self.root / "main.py"), "--project_path", str(self.root / "missing"),
            "--config_name", "japan_web", "--build_mode", "release", "--censor_switch", "false",
        ], capture_output=True, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("渠道配置文件不存在", result.stderr.decode("utf-8"))

    def test_failure_is_not_reported_as_success(self):
        with patch.object(self.lib, "npm_command", return_value=["node", "npm-cli.js"]), \
                patch.object(self.lib.subprocess, "run", return_value=subprocess.CompletedProcess([], 7)):
            with self.assertRaises(SystemExit) as caught:
                self.lib.process_person(self.person)
            self.assertEqual(caught.exception.code, 7)

    def test_missing_config_never_starts_exporter(self):
        (self.project / "python/config/channel.json").unlink()
        with patch.object(self.lib.subprocess, "run") as run:
            with self.assertRaises(SystemExit):
                self.lib.process_person(self.person)
            run.assert_not_called()

    def test_export_contract_and_no_embedded_credentials(self):
        self.assertEqual(PAYLOAD["schemaVersion"], 1)
        params = {p["name"]: p for p in PAYLOAD["prep"]["params"]}
        self.assertNotIn("project_path", params)
        self.assertEqual(params["build_mode"]["fixedValue"], "${build_mode}")
        self.assertEqual(params["censor_switch"]["fixedValue"], "${review_mode}")
        self.assertEqual(len(params["config_name"]["options"]), 7)
        for item in PAYLOAD["files"]:
            self.assertNotRegex(item["content"], r"https?://[^\s/]+:[^\s/]+@")
            self.assertNotIn("git_password", item["content"])
            if item["path"].endswith(".py"):
                compile(item["content"], item["path"], "exec")


if __name__ == "__main__":
    unittest.main()

"""Cold/warm npm execution using unpublished packages and an isolated local registry.

Usage: python -X utf8 scripts/test-config-export-prep-integration.py <export_config directory>
The fixture registry serves only the locally built Windows package. Nothing is published.
"""

import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
from urllib.parse import unquote


def run(args, cwd, env=None):
    result = subprocess.run(args, cwd=cwd, env=env, capture_output=True,
                            encoding="utf-8", errors="replace", timeout=120)
    if result.returncode:
        raise RuntimeError(f"Fixture command failed ({result.returncode}):\n{result.stdout}\n{result.stderr}")
    return result.stdout


def main():
    exporter = Path(sys.argv[1]).resolve()
    node = shutil.which("node")
    uv = shutil.which("uv")
    if os.name != "nt" or not node or not uv:
        raise RuntimeError("This fixture requires Windows x64, Node/npm and uv")
    stage = json.loads(run([
        node, "-e",
        "const r=require('./npm/release.cjs'); const p=r.stagePlatform(); "
        "const m=r.stageMeta(); console.log(JSON.stringify([p.directory,m.directory]));",
    ], exporter).strip().splitlines()[-1])
    payload_path = Path(__file__).resolve().parents[1] / "examples/config-export-prep.json"
    payload = json.loads(payload_path.read_text(encoding="utf-8"))

    with tempfile.TemporaryDirectory(prefix="npm 配置验收 ") as temporary:
        root = Path(temporary)
        packages = {}
        tarballs = {}
        for index, directory in enumerate(stage):
            copied = root / f"package-{index}"
            shutil.copytree(directory, copied)
            manifest = json.loads((copied / "package.json").read_text(encoding="utf-8"))
            if "optionalDependencies" in manifest:
                manifest["optionalDependencies"].pop("@tsixi/export-config-darwin-arm64")
                (copied / "package.json").write_text(json.dumps(manifest), encoding="utf-8")
            archive = root / f"{index}.tgz"
            with tarfile.open(archive, "w:gz") as tar:
                tar.add(copied, arcname="package")
            tarballs[f"/{index}.tgz"] = archive.read_bytes()
            packages[manifest["name"]] = (manifest, f"/{index}.tgz")

        requests = []

        class Registry(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                requests.append(self.path)
                name = unquote(self.path).lstrip("/")
                if self.path in tarballs:
                    body = tarballs[self.path]
                elif name in packages:
                    manifest, archive = packages[name]
                    blob = tarballs[archive]
                    version = dict(manifest, dist={
                        "tarball": f"http://127.0.0.1:{self.server.server_port}{archive}",
                        "integrity": "sha512-" + base64.b64encode(hashlib.sha512(blob).digest()).decode(),
                    })
                    body = json.dumps({"name": name, "dist-tags": {"latest": manifest["version"]},
                                       "versions": {manifest["version"]: version}}).encode()
                elif name == "exec":
                    # Wrongly invoking npx-cli.js with an extra 'exec' attempts to
                    # infer a bin from this package, reproducing the production error.
                    body = json.dumps({"name": "exec", "dist-tags": {"latest": "1.0.0"},
                                       "versions": {"1.0.0": {"name": "exec", "version": "1.0.0"}}}).encode()
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream" if self.path.endswith(".tgz") else "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = ThreadingHTTPServer(("127.0.0.1", 0), Registry)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            repo = root / "remote 配置"
            (repo / "cehua/h5_table_config").mkdir(parents=True)
            (repo / "cehua/h5_table_config/table_name_config.json").write_text("{}")
            (repo / "cehua/text.csv").write_text("key,(en)\nGREETING,cold start\n", encoding="utf-8")
            for filename in ("屏蔽字库.csv", "禁用字库.csv"):
                (repo / "cehua" / filename).write_text("word\nexample\n", encoding="utf-8")
            for args in (["init", "-b", "master"], ["add", "."],
                         ["-c", "user.name=Fixture", "-c", "user.email=fixture@localhost",
                          "-c", "commit.gpgsign=false", "commit", "-m", "fixture"]):
                run(["git", *args], repo)
            project = root / "游戏 project"
            (project / "python/config").mkdir(parents=True)
            (project / "assets").mkdir()
            (project / "python/exportConfig.py").write_text("# marker only")
            (project / "python/config/channel.json").write_text(json.dumps({
                "japan_web": {"lan": ["en"], "configGit": str(repo)},
            }), encoding="utf-8")
            prep = root / "准备 project"
            prep.mkdir()
            for item in payload["files"]:
                (prep / item["path"]).write_text(item["content"], encoding="utf-8")
            (prep / "fixture_entry.py").write_text(
                "import sys\nimport lib\nlib.REGISTRY = sys.argv.pop(1)\nimport main\nmain.main()\n",
                encoding="utf-8",
            )
            # Isolate user/global scope registry mappings as well as the cache.
            for filename in ("user.npmrc", "global.npmrc"):
                (root / filename).write_text("")
            env = dict(os.environ, COCOS_BUILD_LAN_NODE_EXE=node,
                       npm_execpath=str(Path(node).resolve().parent / "node_modules/npm/bin/npx-cli.js"),
                       npm_config_registry=f"http://127.0.0.1:{server.server_port}",
                       npm_config_cache=str(root / "fresh npm cache"),
                       npm_config_userconfig=str(root / "user.npmrc"),
                       npm_config_globalconfig=str(root / "global.npmrc"),
                       npm_config_fetch_retries="0", PYTHONIOENCODING="utf-8")
            args = [uv, "run", "--python", "3.14", "fixture_entry.py",
                    f"http://127.0.0.1:{server.server_port}", "--project_path", str(project),
                    "--config_name", "japan_web", "--build_mode", "release",
                    "--censor_switch", "false", "--release_branch", "master"]
            first = run(args, prep, env)
            output = project / "assets/resources/i18n/en/lang.json"
            original = output.read_bytes()
            assert "cold start" in original.decode()
            assert any(path.endswith(".tgz") for path in requests), requests
            assert "/exec" not in requests, requests
            assert not (project / "node_modules").exists()
            assert "@tsixi/export-config@0.1.2" in first

            server.shutdown()
            server.server_close()
            thread.join()
            env["npm_config_offline"] = "true"
            output.write_text("overwritten", encoding="utf-8")
            second = run(args, prep, env)
            assert output.read_bytes() == original
            assert "--force-export" in second
            output.unlink()
            run(args, prep, env)
            assert output.read_bytes() == original
            print("PASS: uv -> Python -> npm -> Rust -> Git, cold npm cache and offline warm cache")
            print("PASS: Chinese/space paths, clone, forced restore and inherited output")
        finally:
            if thread.is_alive():
                server.shutdown()
                server.server_close()
                thread.join()


if __name__ == "__main__":
    main()

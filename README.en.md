# cocos-build-lan

[中文](README.md)

This generated repository is fully yours. Its immutable UUID is written to `tool.json` once, while all Rust source, business configuration, and controller UI remain local and editable.

> Before using an agent or starting development, read [AGENTS.md](AGENTS.md). It routes each task to the smallest relevant guide and crate; see [AGENTS.en.md](AGENTS.en.md) for English.

## Run it

Rust 1.97.1 is required.

```bash
cargo build -p cocos-build-lan-server
cargo run -p cocos-build-lan-control
```

The native window title is `cocos-build-lan 控制端`. The controller adopts a matching healthy service or starts its sibling server; stop and restart use the real readiness → prepare → shutdown protocol and never hard-kill an unverified process.

To run only the Web tool, build the SPA with dx first, then start the server:

```bash
# Build the Web SPA (requires dioxus-cli: cargo install dioxus-cli --locked)
scripts/build-web.sh

cargo run -p cocos-build-lan-server
# Open http://127.0.0.1:8765
```

In development mode the server serves pages from `web/` next to the executable, i.e. `target/debug/web`; symlink it to the dx output once: `ln -sfn ../dx/cocos-build-lan/release/web/public target/debug/web`. Without a built SPA the homepage only shows a placeholder text.

For Web UI development, prefer the hot-reload workflow (rebuilds automatically on changes to `crates/tool-app/src`):

```bash
# Terminal 1: the server (or the controller: cargo run -p cocos-build-lan-control)
cargo run -p cocos-build-lan-server

# Terminal 2: dx hot reload, open http://127.0.0.1:8080
dx serve --package cocos-build-lan-app
```

`Dioxus.toml` already declares `[[web.proxy]]`, so the dx dev server proxies `/api/*` to the real server on 8765.

## Customize locally

- `tool-contract`: `ToolSettings`, `ToolStatus`, and shared business types.
- `tool-server`: APIs, persistence, business state, and restart guards.
- `tool-control`: all Dioxus Desktop pages, navigation, forms, business cards, and themes.
- `tool-app`: the browser business UI.
- `tool-core`: non-UI runtime, updater, supervisor, and LAN Dev implementation.

Extend settings/status in `tool-contract`, expose them through `tool-server`, and render them in `tool-control`. `tool_id` is immutable and cannot be changed by the configuration endpoint.

See [docs/development.en.md](docs/development.en.md) for the step-by-step business extension path.

## Updates and safety

`ToolSettings` is a nested typed contract: `update` owns the release URL, automatic apply and LAN Dev toggle; `business` owns project fields. The control app uses a form, not a JSON editor. During development there is no migration path: remove an old local settings file and restart to recreate defaults.

The payload is a current-platform ZIP bundle containing both the server and control binaries. Release and LAN Dev share validation, staging and atomic `active.json` switching. A stable launcher owns production startup: after the service is Ready and the form has no unsaved changes, the control app exits, the launcher confirms it has exited, safely stops the server, switches versions, health-checks it, and starts the new control app. On failure it restores the prior version and control app.

LAN Dev is opt-in through a saved `update.lan_dev_enabled`; it runs one background receiver that filters source, tool ID, platform, bundle format, and duplicate broadcasts before downloading the complete bundle. It never distributes source or remote-control messages. The development protocol is intentionally breaking until the stable release. It assumes a trusted LAN; add authentication, signatures, encryption, and access control before using it on an untrusted network.

Run `cargo fmt --check`, `cargo test --workspace`, `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo metadata --format-version 1` before publishing changes.

# Development

See [development.md](development.md). Keep typed local settings in `tool-contract`, machine-only APIs in `tool-server/src/machine.rs`, and their forms in `tool-app/src/pages/machine.rs`. Public settings cannot update engine paths or Git credentials. Every long operation needs an activity guard, and subprocesses use the managed command wrapper. Never return credentials from read endpoints.

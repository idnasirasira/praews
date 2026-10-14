# praews

Portable local web server environment (Nginx + PHP + MariaDB) built with Rust — **zero system pollution**. Everything lives inside a sandbox folder; delete the folder and the machine is clean again.

**v1.0 (macOS first):** Apple Silicon is the primary release target. Windows and Intel Mac MariaDB downloads can follow.

## Workspace

| Crate | Role |
|-------|------|
| `crates/core_engine` | Library + `praews` CLI: sandbox init, Tera config engine, process orchestrator |
| `crates/gui` | Tauri v2 + Vue 3 dashboard + system tray (`frontend/`) |

## Quick start

```bash
cargo build -p core_engine

# Create sandbox layout + seed templates
cargo run -p core_engine -- --root ./sandbox init

# Render nginx.conf / php.ini / my.cnf with absolute portable paths
cargo run -p core_engine -- --root ./sandbox render --php-version 8.3

# CA bundle + local Root CA for *.test HTTPS
cargo run -p core_engine -- --root ./sandbox ensure-cacert
cargo run -p core_engine -- --root ./sandbox ensure-ca

# Add a virtual host
cargo run -p core_engine -- --root ./sandbox add-site myapp.test --path ./sandbox/htdocs

# Local DNS for *.test (port 53535; pair with macOS /etc/resolver/test if desired)
cargo run -p core_engine -- --root ./sandbox dns

# Install portable binaries (Windows + macOS arm64 URLs in manifest)
cargo run -p core_engine -- --root ./sandbox install nginx

# GUI (Tauri + Vue 3) — uses ./dev_sandbox in debug builds
# First time: npm install --prefix crates/gui/frontend && npm run build --prefix crates/gui/frontend
cargo run -p gui

# Hot-reload frontend (starts Vite on :1420). Requires: cargo install tauri-cli
# cargo tauri dev --manifest-path crates/gui/Cargo.toml
```


## Principles

- Never write to OS system dirs, registries, or global env vars
- Normalize paths to forward slashes before injecting into Nginx/PHP/MySQL configs
- Graceful shutdown + process-tree kill to avoid orphan workers

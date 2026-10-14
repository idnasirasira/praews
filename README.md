# praews

Portable local web server environment (Nginx + PHP + MariaDB) built with Rust — a MAMP-style alternative with **zero system pollution**. Everything lives inside a sandbox folder; delete the folder and the machine is clean again.

## Workspace

| Crate | Role |
|-------|------|
| `crates/core_engine` | Library + `praews` CLI: sandbox init, Tera config engine, process orchestrator |
| `crates/gui` | Placeholder for Tauri v2 (Phase 5) |

## Quick start (Phase 1)

```bash
cargo build -p core_engine

# Create sandbox layout + seed templates
cargo run -p core_engine -- --root ./sandbox init

# Render nginx.conf / php.ini / my.cnf with absolute portable paths
cargo run -p core_engine -- --root ./sandbox render --php-version 8.3

# Check ports
cargo run -p core_engine -- ports
```

Starting services requires portable binaries under `sandbox/bin/` (Phase 3 downloader).

## Principles

- Never write to OS system dirs, registries, or global env vars
- Normalize paths to forward slashes before injecting into Nginx/PHP/MySQL configs
- Graceful shutdown + process-tree kill to avoid orphan workers

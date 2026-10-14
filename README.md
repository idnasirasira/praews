# praews

Portable local web server environment (Nginx + PHP + MariaDB) built with Rust — a MAMP-style alternative with **zero system pollution**. Everything lives inside a sandbox folder; delete the folder and the machine is clean again.

## Workspace

| Crate | Role |
|-------|------|
| `crates/core_engine` | Library + `praews` CLI: sandbox init, Tera config engine, process orchestrator |
| `crates/gui` | Placeholder for Tauri v2 (Phase 5) |

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

# Install portable binaries (Windows URLs ready in manifest; macOS needs archive URLs)
cargo run -p core_engine -- --root ./sandbox install nginx
```

## Principles

- Never write to OS system dirs, registries, or global env vars
- Normalize paths to forward slashes before injecting into Nginx/PHP/MySQL configs
- Graceful shutdown + process-tree kill to avoid orphan workers

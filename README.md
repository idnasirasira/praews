# praews

**Portable Nginx + PHP + MariaDB for local development — zero system pollution.**

All binaries, configs, databases, certificates, and logs live inside a single sandbox folder. Move it, back it up, or delete it and your machine stays clean.

[![Release](https://img.shields.io/github/v/release/idnasirasira/praews?include_prereleases&label=release)](https://github.com/idnasirasira/praews/releases)

---

## Highlights

- **Portable sandbox** — `bin/`, `conf/`, `data/`, `logs/`, `htdocs/`, and `state.json` live under one root directory (relocatable from **Tools → Sandbox root**).
- **One-click stack & menu-bar control** — start/stop Nginx, PHP-FPM, and MariaDB from the dashboard or the macOS menu bar tray with a live stack status indicator.
- **Multi-PHP catalog** — download bundled PHP versions (`8.2`, `8.3`, `8.4`) or link any local/Homebrew PHP (`7.4`, `8.1`, etc.) and switch in one click.
- **Automatic `*.test` sites + local HTTPS** — add a project folder, get an Nginx vhost, a locally-issued certificate, and `*.test` resolution via the built-in DNS server.
- **3-layer Config Editor** — visual tuning (`state.json`), raw overrides (`conf/custom/*`) that survive template re-renders, and automatic `nginx -t` rollback on syntax errors.
- **Mobile & LAN preview** — `sslip.io` URLs and a downloadable `/rootCA.crt` so phones on the same Wi-Fi get trusted HTTPS.
- **Scoped project terminal** — opens a shell with the active PHP, Composer shim, and `PHPRC` pre-wired without touching your global shell profile.
- **Signed in-app updates** — check and install new releases from **Tools → Check for updates**.

---

## Platform support

| Platform | Status | Notes |
|----------|--------|-------|
| **macOS (Apple Silicon, arm64)** | **Supported (v1.0)** | Primary target — `.dmg` installer + one-click Nginx, PHP, and MariaDB downloads |
| **Windows (x64)** | Experimental | Portable manifest URLs present; packaged installer coming later |
| **macOS (Intel, x86_64)** | Partial | Nginx & PHP downloads available; bring your own MariaDB binary |

---

## Install (macOS Apple Silicon)

1. Download the latest **`praews_x.y.z_aarch64.dmg`** from [**Releases**](https://github.com/idnasirasira/praews/releases).
2. Open the `.dmg` and drag **praews** to **Applications**.
3. If macOS Gatekeeper warns on first launch (unsigned build):
   ```bash
   xattr -cr /Applications/praews.app
   ```
4. Launch **praews** → complete the first-run **Setup** wizard → click **Start all**.

### Optional one-time system helpers

`praews` never writes outside its sandbox automatically. Two optional actions ask for administrator confirmation once:

- **`*.test` DNS on macOS** — writes `/etc/resolver/test` pointing at `127.0.0.1:<dns_port>` so browsers resolve `*.test` without editing `/etc/hosts`.
- **Local Root CA trust** — adds `conf/ssl/rootCA.crt` to the OS keychain so `https://*.test` shows a green lock.

---

## Sandbox layout

```text
<sandbox_root>/
├── bin/            # nginx, php/<ver>, mysql, composer shims
├── conf/           # Rendered nginx.conf, php.ini, my.cnf
│   ├── custom/     # User raw overrides (never overwritten by templates)
│   ├── ssl/        # Local Root CA + per-site certs
│   └── vhosts/     # Per-site *.test Nginx configs
├── data/mysql/     # MariaDB datadir
├── htdocs/         # Default localhost document root
├── logs/           # nginx, php, mysql, and praews logs
├── temp/           # PID files, sockets, uploads, sessions
├── templates/      # Tera config templates
└── state.json      # Ports, active PHP, sites, and visual config
```

---

## Development

### Prerequisites

- **Rust** (stable, 1.80+)
- **Node.js** 20+ and `npm`
- **Tauri CLI v2** (`cargo install tauri-cli`)

### Run from source

```bash
# Build frontend assets once (required before `cargo run -p gui`)
npm install --prefix crates/gui/frontend
npm run build --prefix crates/gui/frontend

# Launch the desktop app (uses ./dev_sandbox in debug builds)
cargo run -p gui

# Or run with Vite hot-reload
cargo tauri dev
```

### Build a macOS release bundle

```bash
cd crates/gui
cargo tauri build -b app dmg
```

### CLI (`praews`)

The core engine also ships a standalone CLI for headless/scripted workflows:

```bash
cargo run -p core_engine -- --root ./sandbox init
cargo run -p core_engine -- --root ./sandbox install nginx
cargo run -p core_engine -- --root ./sandbox install php --php-version 8.3
cargo run -p core_engine -- --root ./sandbox install mariadb
cargo run -p core_engine -- --root ./sandbox render --php-version 8.3
cargo run -p core_engine -- --root ./sandbox add-site myapp.test --path ./sandbox/htdocs
cargo run -p core_engine -- --root ./sandbox verify-portable
```

---

## Releasing a new version

1. Bump the version in `Cargo.toml`, `crates/gui/tauri.conf.json`, and `crates/gui/frontend/package.json`.
2. Commit, tag, and push:
   ```bash
   git tag -a v1.0.1 -m "praews 1.0.1"
   git push origin master --tags
   ```
3. GitHub Actions builds the signed `.app`, `.dmg`, and `latest.json` and creates a draft Release.

---

## License

MIT

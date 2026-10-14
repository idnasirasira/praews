use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WindowEvent,
};
use tokio::sync::Mutex;

use core_engine::{
    add_site, detect_homebrew_php, ensure_macos_test_resolver, get_local_lan_ip, init_sandbox,
    macos_test_resolver_ok, php_home_is_valid, php_platform_info, relocate_sandbox,
    render_all_configs, render_all_vhosts, resolve_php_home, validate_custom_php_home,
    verify_portable_layout, BinaryManager, CliEnvManager, ComponentSpec, ConfigEditorManager,
    ConfigTarget, LocalDnsServer, MySqlVisualSettings, NginxVisualSettings, PhpExtensionInfo,
    PhpVisualSettings, ProcessOrchestrator, RelocateMode, SandboxConfig, ServiceStatus,
    ServiceType, SslManager,
};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct VHostEntry {
    pub domain: String,
    pub project_path: String,
    pub public_path: String,
    pub ssl_enabled: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct PersistentSettings {
    pub active_php_version: String,
    pub http_port: u16,
    pub https_port: u16,
    pub mysql_port: u16,
    pub php_fastcgi_port: u16,
    pub dns_port: u16,
    pub vhosts: Vec<VHostEntry>,
    /// When true, default localhost nginx root uses `custom_htdocs_path` instead of `<sandbox>/htdocs`.
    #[serde(default)]
    pub custom_htdocs_enabled: bool,
    #[serde(default)]
    pub custom_htdocs_path: Option<String>,
    /// Linked PHP homes (e.g. Homebrew `php@7.4`) keyed by catalog id `"7.4"`.
    #[serde(default)]
    pub custom_php_paths: HashMap<String, String>,
    /// User-declared PHP labels (any X.Y) that are not in the download manifest.
    #[serde(default)]
    pub extra_php_versions: Vec<String>,
    /// Visual quick-tuning persisted in state.json (Layer 1).
    #[serde(default)]
    pub php_settings: PhpVisualSettings,
    #[serde(default)]
    pub mysql_settings: MySqlVisualSettings,
    #[serde(default)]
    pub nginx_settings: NginxVisualSettings,
}

impl Default for PersistentSettings {
    fn default() -> Self {
        Self {
            active_php_version: "8.3".to_string(),
            http_port: 8888,
            https_port: 8443,
            mysql_port: 3306,
            php_fastcgi_port: 9000,
            dns_port: if cfg!(target_os = "macos") { 53535 } else { 53 },
            vhosts: Vec::new(),
            custom_htdocs_enabled: false,
            custom_htdocs_path: None,
            custom_php_paths: HashMap::new(),
            extra_php_versions: Vec::new(),
            php_settings: PhpVisualSettings::default(),
            mysql_settings: MySqlVisualSettings::default(),
            nginx_settings: NginxVisualSettings::default(),
        }
    }
}

impl PersistentSettings {
    pub fn load(app_root: &Path) -> Self {
        let state_file = app_root.join("state.json");
        if let Ok(content) = fs::read_to_string(&state_file) {
            if let Ok(parsed) = serde_json::from_str::<Self>(&content) {
                return parsed;
            }
        }
        let default_settings = Self::default();
        let _ = default_settings.save(app_root);
        default_settings
    }

    pub fn save(&self, app_root: &Path) -> Result<(), String> {
        let state_file = app_root.join("state.json");
        let content = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(state_file, content).map_err(|e| e.to_string())
    }

    pub fn effective_htdocs(&self, app_root: &Path) -> PathBuf {
        if self.custom_htdocs_enabled {
            if let Some(path) = self
                .custom_htdocs_path
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                return PathBuf::from(path);
            }
        }
        app_root.join("htdocs")
    }

    pub fn sandbox_config(&self, app_root: &Path) -> SandboxConfig {
        SandboxConfig::new(app_root, &self.active_php_version)
            .with_ports(self.http_port, self.php_fastcgi_port, self.mysql_port)
            .with_https_port(self.https_port)
            .with_htdocs_root(&self.effective_htdocs(app_root))
            .with_visual_settings(
                self.php_settings.clone(),
                self.mysql_settings.clone(),
                self.nginx_settings.clone(),
            )
    }

    pub fn php_home(&self, app_root: &Path, version: &str) -> PathBuf {
        resolve_php_home(app_root, version, &self.custom_php_paths)
    }

    pub fn php_ready(&self, app_root: &Path, binary_manager: &BinaryManager, version: &str) -> bool {
        let home = self.php_home(app_root, version);
        if self.custom_php_paths.contains_key(version) {
            return php_home_is_valid(&home);
        }
        binary_manager.is_php_installed(version) || php_home_is_valid(&home)
    }
}

pub struct AppState {
    pub app_root: PathBuf,
    pub settings: Mutex<PersistentSettings>,
    pub orchestrator: Arc<ProcessOrchestrator>,
    pub dns_server: Mutex<LocalDnsServer>,
    pub binary_manager: Arc<BinaryManager>,
    pub ssl_manager: Arc<SslManager>,
    pub cli_env: Arc<CliEnvManager>,
    pub config_editor: Arc<ConfigEditorManager>,
}

impl AppState {
    pub async fn build_sandbox_config(&self) -> SandboxConfig {
        let settings = self.settings.lock().await.clone();
        settings.sandbox_config(&self.app_root).with_custom_raw(
            self.config_editor.read_custom_raw(ConfigTarget::Php),
            self.config_editor.read_custom_raw(ConfigTarget::MySql),
            self.config_editor.read_custom_raw(ConfigTarget::Nginx),
        )
    }

    /// Restart only the service whose config just changed (if it was running).
    pub async fn reload_single_service(&self, target: ConfigTarget) -> Result<(), String> {
        let statuses = self.orchestrator.get_all_statuses().await;
        let settings = self.settings.lock().await.clone();

        match target {
            ConfigTarget::Php => {
                if matches!(
                    statuses.get(&ServiceType::Php),
                    Some(ServiceStatus::Running { .. })
                ) {
                    self.orchestrator
                        .stop_service(ServiceType::Php)
                        .await
                        .map_err(|e| e.to_string())?;
                    let php_home =
                        settings.php_home(&self.app_root, &settings.active_php_version);
                    self.orchestrator
                        .start_php_at(&php_home, settings.php_fastcgi_port)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            ConfigTarget::Nginx => {
                if matches!(
                    statuses.get(&ServiceType::Nginx),
                    Some(ServiceStatus::Running { .. })
                ) {
                    self.orchestrator
                        .stop_service(ServiceType::Nginx)
                        .await
                        .map_err(|e| e.to_string())?;
                    self.orchestrator
                        .start_nginx(settings.http_port)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            ConfigTarget::MySql => {
                if matches!(
                    statuses.get(&ServiceType::MySql),
                    Some(ServiceStatus::Running { .. })
                ) {
                    self.orchestrator
                        .stop_service(ServiceType::MySql)
                        .await
                        .map_err(|e| e.to_string())?;
                    self.orchestrator
                        .start_mysql(settings.mysql_port)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
}

/// Where we persist the user-chosen sandbox folder (outside the sandbox itself).
fn root_pointer_path() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(".praews-root")
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("praews.root")))
            .unwrap_or_else(|| PathBuf::from("praews.root"))
    }
}

fn default_app_root() -> PathBuf {
    if cfg!(debug_assertions) {
        let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("dev_sandbox");
        let _ = fs::create_dir_all(&dev_dir);
        fs::canonicalize(&dev_dir).unwrap_or(dev_dir)
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn read_root_pointer() -> Option<PathBuf> {
    let pointer = root_pointer_path();
    let content = fs::read_to_string(&pointer).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    Some(path)
}

fn write_root_pointer(root: &Path) -> Result<(), String> {
    let pointer = root_pointer_path();
    if let Some(parent) = pointer.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    fs::write(&pointer, format!("{}\n", abs.display())).map_err(|e| e.to_string())?;
    Ok(())
}

fn resolve_portable_app_root() -> PathBuf {
    if let Some(from_pointer) = read_root_pointer() {
        let _ = fs::create_dir_all(&from_pointer);
        return fs::canonicalize(&from_pointer).unwrap_or(from_pointer);
    }
    default_app_root()
}

#[tauri::command]
async fn get_app_info(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().await.clone();
    let lan_ip = get_local_lan_ip();
    let mobile_url = format_host_url("http", &format!("praews.{lan_ip}.sslip.io"), settings.http_port);
    let root_ca_url = format!(
        "{}/rootCA.crt",
        format_host_url("http", &lan_ip.to_string(), settings.http_port)
    );
    let php_catalog = build_php_catalog(&state, &settings);
    let php_versions = list_all_php_ids(&state, &settings);
    let installed_php_versions: Vec<String> = php_catalog
        .iter()
        .filter(|e| e.installed)
        .map(|e| e.id.clone())
        .collect();
    let php_platform = php_platform_info(state.binary_manager.manifest());
    Ok(serde_json::json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "app_root": state.app_root.to_string_lossy(),
        "default_app_root": default_app_root().to_string_lossy(),
        "root_pointer": root_pointer_path().to_string_lossy(),
        "using_custom_root": read_root_pointer().is_some(),
        "lan_ip": lan_ip.to_string(),
        "mobile_url": mobile_url,
        "root_ca_url": root_ca_url,
        "dns_hint": format!(
            "Phone Wi-Fi DNS → {} (port {}). Or open {}",
            lan_ip, settings.dns_port, mobile_url
        ),
        "macos_resolver_ok": macos_test_resolver_ok(settings.dns_port),
        "dns_port": settings.dns_port,
        "available_php_versions": php_versions,
        "installed_php_versions": installed_php_versions,
        "php_catalog": php_catalog,
        "php_platform": php_platform,
        "nginx_installed": state.binary_manager.is_nginx_installed(),
        "mariadb_installed": state.binary_manager.is_mariadb_installed(),
        "htdocs_root": settings.effective_htdocs(&state.app_root).to_string_lossy(),
        "settings": settings,
    }))
}

const LOG_FILE_NAMES: &[&str] = &[
    "praews.log",
    "nginx_error.log",
    "php_error.log",
    "php-fpm.log",
    "mysql_error.log",
    "mysql_slow.log",
    "mysql_query.log",
    "nginx_process.log",
    "php_process.log",
    "mysql_process.log",
];

#[derive(Clone, Copy)]
struct LogSourceDef {
    id: &'static str,
    label: &'static str,
    files: &'static [&'static str],
}

const LOG_SOURCES: &[LogSourceDef] = &[
    LogSourceDef {
        id: "all",
        label: "All",
        files: LOG_FILE_NAMES,
    },
    LogSourceDef {
        id: "praews",
        label: "praews",
        files: &["praews.log"],
    },
    LogSourceDef {
        id: "nginx",
        label: "Nginx",
        files: &["nginx_error.log", "nginx_process.log"],
    },
    LogSourceDef {
        id: "php",
        label: "PHP",
        files: &["php_error.log", "php-fpm.log", "php_process.log"],
    },
    LogSourceDef {
        id: "mysql",
        label: "MariaDB",
        files: &[
            "mysql_error.log",
            "mysql_slow.log",
            "mysql_query.log",
            "mysql_process.log",
        ],
    },
];

fn read_log_tail(log_dir: &Path, files: &[&str], lines_each: usize) -> String {
    let mut out = String::new();
    let multi = files.len() > 1;
    for name in files {
        let path = log_dir.join(name);
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        let tail: Vec<&str> = content.lines().rev().take(lines_each).collect();
        if tail.is_empty() {
            continue;
        }
        if multi {
            out.push_str(&format!("===== {name} =====\n"));
        }
        for line in tail.into_iter().rev() {
            out.push_str(line);
            out.push('\n');
        }
        if multi {
            out.push('\n');
        }
    }
    if out.is_empty() {
        "(no log output yet)".into()
    } else {
        out
    }
}

#[tauri::command]
async fn read_recent_logs(
    state: State<'_, AppState>,
    lines: Option<usize>,
    source: Option<String>,
) -> Result<serde_json::Value, String> {
    let n = lines.unwrap_or(120);
    let source_id = source.unwrap_or_else(|| "all".into());
    let def = LOG_SOURCES
        .iter()
        .find(|s| s.id == source_id)
        .unwrap_or(&LOG_SOURCES[0]);
    let log_dir = state.app_root.join("logs");
    let per_file = if def.id == "all" {
        (n / def.files.len().max(1)).max(20)
    } else {
        n
    };
    let content = read_log_tail(&log_dir, def.files, per_file);
    let sources: Vec<serde_json::Value> = LOG_SOURCES
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "label": s.label,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "source": def.id,
        "sources": sources,
        "content": content,
    }))
}

#[tauri::command]
async fn change_app_root(
    app: AppHandle,
    new_root: String,
    mode: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let trimmed = new_root.trim();
    if trimmed.is_empty() {
        return Err("Folder path is required.".into());
    }
    let relocate_mode = match mode.as_str() {
        "point" => RelocateMode::Point,
        "copy" => RelocateMode::Copy,
        "move" => RelocateMode::Move,
        other => {
            return Err(format!(
                "Unknown mode '{other}'. Use point, copy, or move."
            ))
        }
    };

    let old_root = state.app_root.clone();
    let new_path = PathBuf::from(trimmed);

    // Stop stack before touching files.
    {
        let mut dns = state.dns_server.lock().await;
        dns.stop();
    }
    state
        .orchestrator
        .stop_all()
        .await
        .map_err(|e| e.to_string())?;

    let resolved = relocate_sandbox(&old_root, &new_path, relocate_mode).map_err(|e| e.to_string())?;
    write_root_pointer(&resolved)?;

    // Re-render configs under the new absolute paths so nginx/php/mysql stay valid.
    let settings = PersistentSettings::load(&resolved);
    let sandbox_cfg = settings.sandbox_config(&resolved);
    let _ = render_all_configs(&resolved, &sandbox_cfg);
    let vhost_pairs: Vec<(String, String)> = settings
        .vhosts
        .iter()
        .map(|v| (v.domain.clone(), v.public_path.clone()))
        .collect();
    let _ = render_all_vhosts(&resolved, &sandbox_cfg, &vhost_pairs);

    append_praews_log(
        &resolved,
        &format!(
            "Sandbox root changed ({mode}) → {}",
            resolved.display()
        ),
    );

    // Restart so BinaryManager / orchestrator rebuild against the new root.
    let env = app.env();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        tauri::process::restart(&env);
    });

    Ok(serde_json::json!({
        "ok": true,
        "app_root": resolved.to_string_lossy(),
        "mode": mode,
        "restarting": true,
        "message": "Sandbox root updated. App is restarting…",
    }))
}

#[tauri::command]
async fn reset_app_root(app: AppHandle, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    {
        let mut dns = state.dns_server.lock().await;
        dns.stop();
    }
    state
        .orchestrator
        .stop_all()
        .await
        .map_err(|e| e.to_string())?;

    let pointer = root_pointer_path();
    if pointer.exists() {
        fs::remove_file(&pointer).map_err(|e| e.to_string())?;
    }
    let fallback = default_app_root();
    let _ = init_sandbox(&fallback);

    let env = app.env();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        tauri::process::restart(&env);
    });

    Ok(serde_json::json!({
        "ok": true,
        "app_root": fallback.to_string_lossy(),
        "restarting": true,
        "message": "Reverted to default sandbox. App is restarting…",
    }))
}

#[tauri::command]
async fn update_htdocs_root(
    enabled: bool,
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<PersistentSettings, String> {
    let htdocs_path = path
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if enabled {
        let Some(ref p) = htdocs_path else {
            return Err("Folder path is required when custom htdocs is enabled.".into());
        };
        let path_buf = PathBuf::from(p);
        fs::create_dir_all(&path_buf).map_err(|e| format!("cannot create htdocs folder: {e}"))?;
        if !path_buf.is_dir() {
            return Err(format!("Not a folder: {p}"));
        }
    }

    let updated = {
        let mut settings = state.settings.lock().await;
        settings.custom_htdocs_enabled = enabled;
        settings.custom_htdocs_path = htdocs_path.clone();
        settings.save(&state.app_root)?;
        settings.clone()
    };

    let config = updated.sandbox_config(&state.app_root);
    render_all_configs(&state.app_root, &config).map_err(|e| e.to_string())?;

    // Reload nginx if it's running so the new document root applies immediately.
    let statuses = state.orchestrator.get_all_statuses().await;
    if matches!(
        statuses.get(&ServiceType::Nginx),
        Some(ServiceStatus::Running { .. })
    ) {
        let _ = state.orchestrator.stop_service(ServiceType::Nginx).await;
        let _ = state
            .orchestrator
            .start_nginx(updated.http_port)
            .await
            .map_err(|e| e.to_string())?;
    }

    append_praews_log(
        &state.app_root,
        &format!(
            "htdocs root {} → {}",
            if enabled { "enabled" } else { "disabled" },
            updated.effective_htdocs(&state.app_root).display()
        ),
    );

    Ok(updated)
}

#[tauri::command]
async fn clear_logs(state: State<'_, AppState>) -> Result<usize, String> {
    let log_dir = state.app_root.join("logs");
    let _ = fs::create_dir_all(&log_dir);
    let mut cleared = 0usize;

    for name in LOG_FILE_NAMES {
        let path = log_dir.join(name);
        if path.exists() {
            fs::write(&path, "").map_err(|e| format!("failed to clear {name}: {e}"))?;
            cleared += 1;
        }
    }

    // Also truncate any other *.log under logs/ (e.g. future services).
    if let Ok(entries) = fs::read_dir(&log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_known = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| LOG_FILE_NAMES.contains(&n))
                .unwrap_or(false);
            if is_known {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) == Some("log") && path.is_file() {
                fs::write(&path, "").map_err(|e| format!("failed to clear {:?}: {e}", path))?;
                cleared += 1;
            }
        }
    }

    append_praews_log(&state.app_root, "Logs cleared from UI");
    Ok(cleared)
}

#[derive(serde::Serialize, Clone)]
struct SetupComponentStatus {
    id: String,
    label: String,
    installed: bool,
}

fn normalize_php_label(raw: &str) -> Result<String, String> {
    let label = raw.trim().trim_start_matches('v').trim_start_matches('V');
    if label.is_empty() {
        return Err("Enter a version like 7.4 or 5.6.".into());
    }
    if label.len() > 16 {
        return Err("Version label is too long.".into());
    }
    let ok = label.chars().all(|c| c.is_ascii_digit() || c == '.')
        && label.contains('.')
        && !label.starts_with('.')
        && !label.ends_with('.')
        && !label.contains("..");
    if !ok {
        return Err("Use a version like 7.4, 5.6, or 8.1.".into());
    }
    // Prefer major.minor as the catalog id (7.4.33 → 7.4)
    let parts: Vec<&str> = label.split('.').collect();
    if parts.len() >= 2 {
        Ok(format!("{}.{}", parts[0], parts[1]))
    } else {
        Ok(label.to_string())
    }
}

fn list_manifest_php_versions(state: &AppState) -> Vec<String> {
    let mut versions: Vec<String> = state
        .binary_manager
        .manifest()
        .for_current_platform()
        .php
        .keys()
        .cloned()
        .collect();
    versions.sort();
    versions
}

fn list_available_php_versions(state: &AppState) -> Vec<String> {
    list_manifest_php_versions(state)
}

fn list_all_php_ids(state: &AppState, settings: &PersistentSettings) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for v in list_manifest_php_versions(state) {
        set.insert(v);
    }
    for v in &settings.extra_php_versions {
        set.insert(v.clone());
    }
    for v in settings.custom_php_paths.keys() {
        set.insert(v.clone());
    }
    // Sandbox installs that aren't in the manifest yet
    let php_root = state.app_root.join("bin/php");
    if let Ok(entries) = fs::read_dir(&php_root) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                        set.insert(name.to_string());
                    }
                }
            }
        }
    }
    set.into_iter().collect()
}

#[derive(serde::Serialize, Clone)]
struct PhpCatalogEntry {
    id: String,
    version: String,
    installed: bool,
    active: bool,
    downloadable: bool,
    /// `available` | `user` (user-added / linked)
    source: String,
    custom_path: Option<String>,
    detected_homebrew: Option<String>,
    note: Option<String>,
}

fn build_php_catalog(state: &AppState, settings: &PersistentSettings) -> Vec<PhpCatalogEntry> {
    let active = &settings.active_php_version;
    let platform = state.binary_manager.manifest().for_current_platform();
    let ids = list_all_php_ids(state, settings);
    let mut entries: Vec<PhpCatalogEntry> = ids
        .into_iter()
        .map(|id| {
            let spec = platform.php.get(&id);
            let downloadable = spec
                .and_then(|s| s.url.as_ref())
                .map(|u| !u.is_empty())
                .unwrap_or(false)
                && spec.map(|s| s.status != "custom_only").unwrap_or(true);
            let source = if downloadable {
                "available".to_string()
            } else {
                "user".to_string()
            };
            let version = spec
                .map(|s| s.version.clone())
                .unwrap_or_else(|| id.clone());
            let custom_path = settings
                .custom_php_paths
                .get(&id)
                .cloned()
                .filter(|s| !s.trim().is_empty());
            let installed = settings.php_ready(&state.app_root, &state.binary_manager, &id);
            let brew = detect_homebrew_php(&id).map(|p| p.to_string_lossy().into_owned());
            let note = if source == "user" && custom_path.is_none() {
                Some("Add a label, then link a PHP folder (Homebrew or any build).".into())
            } else {
                spec.and_then(|s| s.note.clone())
            };
            PhpCatalogEntry {
                id: id.clone(),
                version,
                installed,
                active: &id == active,
                downloadable,
                source,
                custom_path,
                detected_homebrew: brew,
                note,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries
}

#[tauri::command]
async fn get_setup_status(
    php_version: Option<String>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let versions = list_available_php_versions(&state);
    let active = state.settings.lock().await.active_php_version.clone();
    let php_version = php_version
        .filter(|v| versions.iter().any(|x| x == v))
        .unwrap_or_else(|| {
            if versions.iter().any(|x| x == &active) {
                active.clone()
            } else {
                versions
                    .iter()
                    .find(|v| *v == "8.3")
                    .cloned()
                    .or_else(|| versions.first().cloned())
                    .unwrap_or(active)
            }
        });

    let components = vec![
        SetupComponentStatus {
            id: "nginx".into(),
            label: "Nginx".into(),
            installed: state.binary_manager.is_nginx_installed(),
        },
        SetupComponentStatus {
            id: "php".into(),
            label: format!("PHP {php_version}"),
            installed: state.binary_manager.is_php_installed(&php_version),
        },
        SetupComponentStatus {
            id: "mariadb".into(),
            label: "MariaDB".into(),
            installed: state.binary_manager.is_mariadb_installed(),
        },
    ];
    let ready = components.iter().all(|c| c.installed);
    Ok(serde_json::json!({
        "ready": ready,
        "php_version": php_version,
        "available_php_versions": versions,
        "components": components,
    }))
}

#[tauri::command]
async fn run_first_setup(
    app_handle: AppHandle,
    php_version: Option<String>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let versions = list_available_php_versions(&state);
    let php_ver = {
        let mut settings = state.settings.lock().await;
        if let Some(ver) = php_version {
            if !versions.iter().any(|v| v == &ver) {
                return Err(format!(
                    "PHP {ver} is not available on this platform. Choose one of: {}",
                    versions.join(", ")
                ));
            }
            settings.active_php_version = ver;
            settings.save(&state.app_root)?;
        } else if !versions.iter().any(|v| v == &settings.active_php_version) {
            let fallback = versions
                .iter()
                .find(|v| *v == "8.3")
                .cloned()
                .or_else(|| versions.first().cloned())
                .ok_or_else(|| "No PHP versions in manifest for this platform".to_string())?;
            settings.active_php_version = fallback;
            settings.save(&state.app_root)?;
        }
        settings.active_php_version.clone()
    };

    init_sandbox(&state.app_root).map_err(|e| e.to_string())?;

    let platform = state.binary_manager.manifest().for_current_platform().clone();
    let mut plan: Vec<(String, ComponentSpec)> = Vec::new();

    if !state.binary_manager.is_nginx_installed() {
        plan.push(("nginx".into(), platform.nginx.clone()));
    }
    if !state.binary_manager.is_php_installed(&php_ver) {
        let spec = platform
            .php
            .get(&php_ver)
            .cloned()
            .ok_or_else(|| format!("PHP {php_ver} missing from manifest"))?;
        plan.push(("php".into(), spec));
    }
    if !state.binary_manager.is_mariadb_installed() {
        plan.push(("mariadb".into(), platform.mariadb.clone()));
    }

    let total = plan.len().max(1);
    for (idx, (name, spec)) in plan.iter().enumerate() {
        let step = idx + 1;
        let _ = app_handle.emit(
            "setup-progress",
            serde_json::json!({
                "phase": "install",
                "component": name,
                "step": step,
                "total": total,
                "percentage": 0.0,
                "message": format!("Installing {name} ({step}/{total})…"),
            }),
        );

        let app_clone = app_handle.clone();
        let component = name.clone();
        let step_n = step;
        let total_n = total;
        state
            .binary_manager
            .install_from_spec(name, spec, move |progress| {
                let _ = app_clone.emit("download-progress", &progress);
                let _ = app_clone.emit(
                    "setup-progress",
                    serde_json::json!({
                        "phase": "download",
                        "component": component,
                        "step": step_n,
                        "total": total_n,
                        "percentage": progress.percentage,
                        "message": format!(
                            "{component} · {:.0}% ({step_n}/{total_n})",
                            progress.percentage
                        ),
                    }),
                );
            })
            .await
            .map_err(|e| format!("{name}: {e}"))?;

        let _ = app_handle.emit(
            "setup-progress",
            serde_json::json!({
                "phase": "done_step",
                "component": name,
                "step": step,
                "total": total,
                "percentage": 100.0,
                "message": format!("{name} ready ({step}/{total})"),
            }),
        );
    }

    let _ = app_handle.emit(
        "setup-progress",
        serde_json::json!({
            "phase": "cacert",
            "component": "cacert",
            "step": total,
            "total": total,
            "percentage": 100.0,
            "message": "Fetching CA certificate bundle…",
        }),
    );
    state
        .binary_manager
        .ensure_cacert_bundle()
        .await
        .map_err(|e| e.to_string())?;

    let components = vec![
        SetupComponentStatus {
            id: "nginx".into(),
            label: "Nginx".into(),
            installed: state.binary_manager.is_nginx_installed(),
        },
        SetupComponentStatus {
            id: "php".into(),
            label: format!("PHP {php_ver}"),
            installed: state.binary_manager.is_php_installed(&php_ver),
        },
        SetupComponentStatus {
            id: "mariadb".into(),
            label: "MariaDB".into(),
            installed: state.binary_manager.is_mariadb_installed(),
        },
    ];
    let ready = components.iter().all(|c| c.installed);
    let status = serde_json::json!({
        "ready": ready,
        "php_version": php_ver,
        "components": components,
    });
    let _ = app_handle.emit(
        "setup-progress",
        serde_json::json!({
            "phase": "complete",
            "component": "stack",
            "step": total,
            "total": total,
            "percentage": 100.0,
            "message": "Setup complete. You're ready to go.",
        }),
    );
    Ok(status)
}

#[tauri::command]
async fn install_manifest_component(
    app_handle: AppHandle,
    component: String,
    php_version: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let platform = state.binary_manager.manifest().for_current_platform();
    let (name, spec) = match component.as_str() {
        "nginx" => ("nginx", &platform.nginx),
        "mariadb" | "mysql" => ("mariadb", &platform.mariadb),
        "php" => {
            let ver = php_version.unwrap_or_else(|| "8.3".into());
            let spec = platform
                .php
                .get(&ver)
                .ok_or_else(|| format!("PHP {ver} missing from manifest"))?;
            ("php", spec)
        }
        other => return Err(format!("unknown component: {other}")),
    };
    let app_clone = app_handle.clone();
    state
        .binary_manager
        .install_from_spec(name, spec, move |progress| {
            let _ = app_clone.emit("download-progress", progress);
        })
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn verify_portable(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let report = verify_portable_layout(&state.app_root).map_err(|e| e.to_string())?;
    serde_json::to_value(report).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_service_statuses(
    state: State<'_, AppState>,
) -> Result<HashMap<ServiceType, ServiceStatus>, String> {
    Ok(state.orchestrator.get_all_statuses().await)
}

fn append_praews_log(app_root: &Path, message: &str) {
    let path = app_root.join("logs/praews.log");
    let _ = fs::create_dir_all(app_root.join("logs"));
    let line = format!(
        "[{}] {}\n",
        chrono_like_now(),
        message
    );
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(line.as_bytes())
        });
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

fn parse_service_type(service: &str) -> Result<ServiceType, String> {
    match service {
        "Nginx" | "nginx" => Ok(ServiceType::Nginx),
        "Php" | "PHP" | "php" => Ok(ServiceType::Php),
        "MySql" | "MySQL" | "mysql" | "MariaDB" | "mariadb" => Ok(ServiceType::MySql),
        other => Err(format!("unknown service: {other}")),
    }
}

async fn prepare_runtime(state: &AppState, settings: &PersistentSettings) -> Result<(), String> {
    let root = &state.app_root;
    init_sandbox(root).map_err(|e| e.to_string())?;
    state
        .binary_manager
        .ensure_cacert_bundle()
        .await
        .map_err(|e| e.to_string())?;
    let config = settings.sandbox_config(root);
    render_all_configs(root, &config).map_err(|e| e.to_string())?;
    Ok(())
}

async fn start_one_service(state: &AppState, service: ServiceType) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    let root = state.app_root.clone();
    let label = match service {
        ServiceType::Nginx => "Nginx",
        ServiceType::Php => "PHP",
        ServiceType::MySql => "MariaDB",
    };

    append_praews_log(&root, &format!("Start {label} requested"));

    match service {
        ServiceType::Nginx if !state.binary_manager.is_nginx_installed() => {
            return Err("Nginx belum terpasang — install from Tools.".into());
        }
        ServiceType::Php
            if !settings.php_ready(
                &state.app_root,
                &state.binary_manager,
                &settings.active_php_version,
            ) =>
        {
            return Err(format!(
                "PHP {} belum siap — download or link from the PHP tab.",
                settings.active_php_version
            ));
        }
        ServiceType::MySql if !state.binary_manager.is_mariadb_installed() => {
            return Err("MariaDB belum terpasang — install from Tools.".into());
        }
        _ => {}
    }

    prepare_runtime(state, &settings).await.map_err(|e| {
        append_praews_log(&root, &format!("ERROR: {e}"));
        e
    })?;

    let result = match service {
        ServiceType::MySql => {
            state
                .binary_manager
                .initialize_mysql_datadir()
                .await
                .map_err(|e| format!("MySQL init: {e}"))?;
            state
                .orchestrator
                .start_mysql(settings.mysql_port)
                .await
                .map_err(|e| format!("MySQL: {e}"))
        }
        ServiceType::Php => {
            let php_home = settings.php_home(&state.app_root, &settings.active_php_version);
            state
                .orchestrator
                .start_php_at(&php_home, settings.php_fastcgi_port)
                .await
                .map_err(|e| format!("PHP: {e}"))
        }
        ServiceType::Nginx => state
            .orchestrator
            .start_nginx(settings.http_port)
            .await
            .map_err(|e| format!("Nginx: {e}")),
    };

    if let Err(e) = result {
        append_praews_log(&root, &format!("ERROR: {e}"));
        return Err(e);
    }

    if service == ServiceType::Nginx {
        let mut dns = state.dns_server.lock().await;
        match dns.start().await {
            Ok(()) => append_praews_log(&root, "DNS started"),
            Err(e) => append_praews_log(&root, &format!("DNS warning: {e}")),
        }
        drop(dns);
        match ensure_macos_test_resolver(settings.dns_port) {
            Ok(()) => append_praews_log(
                &root,
                &format!("macOS /etc/resolver/test → 127.0.0.1:{}", settings.dns_port),
            ),
            Err(e) => append_praews_log(&root, &format!("DNS resolver warning: {e}")),
        }
    }

    append_praews_log(&root, &format!("Start {label} completed"));
    Ok(())
}

#[tauri::command]
async fn ensure_dns_resolver(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    ensure_macos_test_resolver(settings.dns_port).map_err(|e| e.to_string())?;
    append_praews_log(
        &state.app_root,
        &format!(
            "macOS /etc/resolver/test fixed → 127.0.0.1:{}",
            settings.dns_port
        ),
    );
    Ok(())
}

#[tauri::command]
async fn start_all_services(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    let root = state.app_root.clone();

    let fail = |msg: String| -> Result<(), String> {
        append_praews_log(&root, &format!("ERROR: {msg}"));
        Err(msg)
    };

    append_praews_log(&root, "Start All requested");

    let mut missing = Vec::new();
    if !state.binary_manager.is_nginx_installed() {
        missing.push("nginx — install from Tools".into());
    }
    if !settings.php_ready(
        &state.app_root,
        &state.binary_manager,
        &settings.active_php_version,
    ) {
        missing.push(format!(
            "PHP {} — download or link from the PHP tab",
            settings.active_php_version
        ));
    }
    if !state.binary_manager.is_mariadb_installed() {
        missing.push("MariaDB — install from Tools".into());
    }
    if !missing.is_empty() {
        let msg = format!(
            "Binaries belum terpasang:\n- {}\nJalankan first-run setup (atau Install Nginx / PHP / MariaDB), baru Start.",
            missing.join("\n- ")
        );
        return fail(msg);
    }

    prepare_runtime(&state, &settings).await.map_err(|e| {
        append_praews_log(&root, &format!("ERROR: {e}"));
        e
    })?;

    start_one_service(&state, ServiceType::MySql).await?;
    start_one_service(&state, ServiceType::Php).await?;
    start_one_service(&state, ServiceType::Nginx).await?;

    append_praews_log(&root, "Start All completed");
    Ok(())
}

#[tauri::command]
async fn start_service(state: State<'_, AppState>, service: String) -> Result<(), String> {
    let service = parse_service_type(&service)?;
    start_one_service(&state, service).await
}

#[tauri::command]
async fn stop_service(state: State<'_, AppState>, service: String) -> Result<(), String> {
    let service = parse_service_type(&service)?;
    let label = match service {
        ServiceType::Nginx => "Nginx",
        ServiceType::Php => "PHP",
        ServiceType::MySql => "MariaDB",
    };
    append_praews_log(&state.app_root, &format!("Stop {label} requested"));

    if service == ServiceType::Nginx {
        let mut dns = state.dns_server.lock().await;
        dns.stop();
    }

    state
        .orchestrator
        .stop_service(service)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn stop_all_services(state: State<'_, AppState>) -> Result<(), String> {
    {
        let mut dns = state.dns_server.lock().await;
        dns.stop();
    }
    state
        .orchestrator
        .stop_all()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn list_php_catalog(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().await.clone();
    Ok(serde_json::json!({
        "catalog": build_php_catalog(&state, &settings),
        "platform": php_platform_info(state.binary_manager.manifest()),
    }))
}

#[tauri::command]
async fn switch_php_version(version: String, state: State<'_, AppState>) -> Result<(), String> {
    let (php_port, php_home) = {
        let mut settings = state.settings.lock().await;
        if !settings.php_ready(&state.app_root, &state.binary_manager, &version) {
            return Err(format!(
                "PHP {version} is not ready. Download it or link a custom folder from the PHP tab."
            ));
        }
        settings.active_php_version = version.clone();
        settings.save(&state.app_root)?;
        let config = settings.sandbox_config(&state.app_root);
        render_all_configs(&state.app_root, &config).map_err(|e| e.to_string())?;
        (
            settings.php_fastcgi_port,
            settings.php_home(&state.app_root, &version),
        )
    };

    let statuses = state.orchestrator.get_all_statuses().await;
    if matches!(
        statuses.get(&ServiceType::Php),
        Some(ServiceStatus::Running { .. })
    ) {
        state
            .orchestrator
            .stop_service(ServiceType::Php)
            .await
            .map_err(|e| e.to_string())?;
        state
            .orchestrator
            .start_php_at(&php_home, php_port)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn add_php_version(version_label: String, state: State<'_, AppState>) -> Result<String, String> {
    let label = normalize_php_label(&version_label)?;
    let in_manifest = list_manifest_php_versions(&state).iter().any(|v| v == &label);
    let mut settings = state.settings.lock().await;
    if !in_manifest && !settings.extra_php_versions.iter().any(|v| v == &label) {
        settings.extra_php_versions.push(label.clone());
        settings.extra_php_versions.sort();
    }
    settings.save(&state.app_root)?;
    Ok(label)
}

#[tauri::command]
async fn register_custom_php_path(
    version_label: String,
    custom_dir_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let label = normalize_php_label(&version_label)?;
    let in_manifest = list_manifest_php_versions(&state).iter().any(|v| v == &label);
    let dir = PathBuf::from(custom_dir_path.trim());
    let version_line = validate_custom_php_home(&dir)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut settings = state.settings.lock().await;
        if !in_manifest && !settings.extra_php_versions.iter().any(|v| v == &label) {
            settings.extra_php_versions.push(label.clone());
            settings.extra_php_versions.sort();
        }
        settings
            .custom_php_paths
            .insert(label.clone(), dir.to_string_lossy().into_owned());
        settings.save(&state.app_root)?;
    }

    Ok(version_line)
}

#[tauri::command]
async fn clear_custom_php_path(
    version_label: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let label = normalize_php_label(&version_label)?;
    let mut settings = state.settings.lock().await;
    if settings.active_php_version == label {
        return Err(format!(
            "Cannot unlink active PHP {label}. Switch to another version first."
        ));
    }
    settings.custom_php_paths.remove(&label);
    settings.save(&state.app_root)?;
    Ok(())
}

#[tauri::command]
async fn remove_php_version(version: String, state: State<'_, AppState>) -> Result<(), String> {
    let label = normalize_php_label(&version)?;
    let active = state.settings.lock().await.active_php_version.clone();
    if label == active {
        return Err(format!(
            "Cannot remove active PHP {label}. Switch to another version first."
        ));
    }

    let mut settings = state.settings.lock().await;
    let had_custom = settings.custom_php_paths.remove(&label).is_some();
    settings.extra_php_versions.retain(|v| v != &label);
    settings.save(&state.app_root)?;
    drop(settings);

    // Only delete sandbox binaries for non-linked installs.
    if !had_custom && state.binary_manager.is_php_installed(&label) {
        state
            .binary_manager
            .remove_php_version(&label)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn download_and_install_binary(
    app_handle: AppHandle,
    component_name: String,
    download_url: String,
    target_rel_dir: String,
    strip_root_folder: bool,
    archive_kind: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let app_clone = app_handle.clone();
    let kind = archive_kind.unwrap_or_else(|| "zip".into());
    state
        .binary_manager
        .install_component(
            &component_name,
            &download_url,
            &target_rel_dir,
            strip_root_folder,
            &kind,
            None,
            move |progress| {
                let _ = app_clone.emit("download-progress", progress);
            },
        )
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize, Clone)]
struct HtdocsFolderEntry {
    name: String,
    path: String,
    suggested_domain: String,
}

#[tauri::command]
async fn list_htdocs_folders(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().await.clone();
    let root = settings.effective_htdocs(&state.app_root);
    let root_display = root.to_string_lossy().into_owned();

    if !root.exists() {
        return Ok(serde_json::json!({
            "htdocs_root": root_display,
            "folders": Vec::<HtdocsFolderEntry>::new(),
        }));
    }

    let mut folders = Vec::new();
    let entries = fs::read_dir(&root).map_err(|e| format!("cannot read htdocs: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let slug = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let suggested = if slug.is_empty() {
            format!("{name}.test")
        } else {
            format!("{slug}.test")
        };
        folders.push(HtdocsFolderEntry {
            name,
            path: path.to_string_lossy().into_owned(),
            suggested_domain: suggested,
        });
    }
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(serde_json::json!({
        "htdocs_root": root_display,
        "folders": folders,
    }))
}

#[tauri::command]
async fn create_virtual_host(
    domain: String,
    project_path: String,
    state: State<'_, AppState>,
) -> Result<VHostEntry, String> {
    let settings = state.settings.lock().await.clone();
    let site = add_site(
        &state.app_root,
        &domain,
        Path::new(&project_path),
        settings.http_port,
        settings.https_port,
        settings.php_fastcgi_port,
    )
    .map_err(|e| e.to_string())?;

    let entry = VHostEntry {
        domain: site.domain.clone(),
        project_path: site.project_path.to_string_lossy().into_owned(),
        public_path: site.public_path.to_string_lossy().into_owned(),
        ssl_enabled: true,
    };

    {
        let mut settings_mut = state.settings.lock().await;
        settings_mut.vhosts.retain(|v| v.domain != domain);
        settings_mut.vhosts.push(entry.clone());
        settings_mut.save(&state.app_root)?;
    }

    let statuses = state.orchestrator.get_all_statuses().await;
    if matches!(
        statuses.get(&ServiceType::Nginx),
        Some(ServiceStatus::Running { .. })
    ) {
        let _ = state.orchestrator.stop_service(ServiceType::Nginx).await;
        let _ = state.orchestrator.start_nginx(settings.http_port).await;
    }

    Ok(entry)
}

#[tauri::command]
async fn trust_local_root_ca(state: State<'_, AppState>) -> Result<(), String> {
    state
        .ssl_manager
        .trust_root_ca_in_os()
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct PortConfigPayload {
    pub http_port: u16,
    pub https_port: u16,
    pub mysql_port: u16,
    pub php_fastcgi_port: u16,
    pub dns_port: u16,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct PortCheckResult {
    pub port: u16,
    pub available: bool,
    pub suggested_alternative: u16,
}

fn format_host_url(scheme: &str, host: &str, port: u16) -> String {
    let omit = matches!((scheme, port), ("http", 80) | ("https", 443));
    if omit {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

fn find_next_available_port(start_port: u16, reserved: &HashSet<u16>) -> u16 {
    let mut port = start_port.max(1);
    while port < u16::MAX {
        if !reserved.contains(&port) && TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
        port += 1;
    }
    start_port
}

fn claim_free_port(preferred: u16, reserved: &mut HashSet<u16>) -> u16 {
    let port = if !reserved.contains(&preferred) && ProcessOrchestrator::is_port_available(preferred)
    {
        preferred
    } else {
        find_next_available_port(preferred.saturating_add(1), reserved)
    };
    reserved.insert(port);
    port
}

#[tauri::command]
async fn check_ports_availability(
    ports: Vec<u16>,
    state: State<'_, AppState>,
) -> Result<Vec<PortCheckResult>, String> {
    let statuses = state.orchestrator.get_all_statuses().await;
    let mut our_own_active_ports = HashSet::new();
    for status in statuses.values() {
        if let ServiceStatus::Running { port, .. } = status {
            our_own_active_ports.insert(*port);
        }
    }
    // Treat configured ports as ours while the stack is up so the Ports UI
    // does not flag nginx/mysql as "in use by another app".
    if !our_own_active_ports.is_empty() {
        let settings = state.settings.lock().await;
        our_own_active_ports.insert(settings.http_port);
        our_own_active_ports.insert(settings.https_port);
        our_own_active_ports.insert(settings.mysql_port);
        our_own_active_ports.insert(settings.php_fastcgi_port);
        our_own_active_ports.insert(settings.dns_port);
    }

    let mut results = Vec::new();
    for p in ports {
        let is_ours = our_own_active_ports.contains(&p);
        let available = is_ours || ProcessOrchestrator::is_port_available(p);
        let suggested_alternative = if available {
            p
        } else {
            find_next_available_port(p.saturating_add(1), &HashSet::new())
        };
        results.push(PortCheckResult {
            port: p,
            available,
            suggested_alternative,
        });
    }
    Ok(results)
}

#[tauri::command]
async fn update_ports(
    new_ports: PortConfigPayload,
    state: State<'_, AppState>,
) -> Result<PersistentSettings, String> {
    let port_list = [
        new_ports.http_port,
        new_ports.https_port,
        new_ports.mysql_port,
        new_ports.php_fastcgi_port,
    ];
    let unique_ports: HashSet<u16> = port_list.iter().copied().collect();
    if unique_ports.len() != port_list.len() {
        return Err(
            "Port HTTP, HTTPS, MySQL, and PHP FastCGI must all be different numbers.".into(),
        );
    }

    let current_statuses = state.orchestrator.get_all_statuses().await;
    let was_running = current_statuses
        .values()
        .any(|s| matches!(s, ServiceStatus::Running { .. }));

    if was_running {
        {
            let mut dns = state.dns_server.lock().await;
            dns.stop();
        }
        state
            .orchestrator
            .stop_all()
            .await
            .map_err(|e| e.to_string())?;
    }

    for (name, p) in [
        ("HTTP (Nginx)", new_ports.http_port),
        ("HTTPS (Nginx SSL)", new_ports.https_port),
        ("MySQL", new_ports.mysql_port),
        ("PHP FastCGI", new_ports.php_fastcgi_port),
    ] {
        if !ProcessOrchestrator::is_port_available(p) {
            let alt = find_next_available_port(p.saturating_add(1), &HashSet::new());
            return Err(format!(
                "Port {p} for {name} is already in use. Suggested free port: {alt}"
            ));
        }
    }

    let updated_settings = {
        let mut settings = state.settings.lock().await;
        settings.http_port = new_ports.http_port;
        settings.https_port = new_ports.https_port;
        settings.mysql_port = new_ports.mysql_port;
        settings.php_fastcgi_port = new_ports.php_fastcgi_port;
        settings.dns_port = new_ports.dns_port;
        settings.save(&state.app_root)?;
        settings.clone()
    };

    {
        let mut dns = state.dns_server.lock().await;
        dns.port = new_ports.dns_port;
    }

    let sandbox_cfg = updated_settings.sandbox_config(&state.app_root);
    render_all_configs(&state.app_root, &sandbox_cfg).map_err(|e| e.to_string())?;

    let vhost_pairs: Vec<(String, String)> = updated_settings
        .vhosts
        .iter()
        .map(|v| (v.domain.clone(), v.public_path.clone()))
        .collect();
    render_all_vhosts(&state.app_root, &sandbox_cfg, &vhost_pairs).map_err(|e| e.to_string())?;

    if was_running {
        start_all_services(state).await?;
    }

    Ok(updated_settings)
}

#[tauri::command]
async fn apply_port_preset(
    preset: String,
    state: State<'_, AppState>,
) -> Result<PersistentSettings, String> {
    let default_dns: u16 = if cfg!(target_os = "macos") { 53535 } else { 53 };

    let payload = match preset.as_str() {
        "standard" => PortConfigPayload {
            http_port: 80,
            https_port: 443,
            mysql_port: 3306,
            php_fastcgi_port: 9000,
            dns_port: default_dns,
        },
        "high_ports" | "mamp_safe" => PortConfigPayload {
            http_port: 8888,
            https_port: 8890,
            mysql_port: 8889,
            php_fastcgi_port: 9009,
            dns_port: 53535,
        },
        "auto_free" => {
            let mut reserved = HashSet::new();
            PortConfigPayload {
                http_port: claim_free_port(80, &mut reserved),
                https_port: claim_free_port(443, &mut reserved),
                mysql_port: claim_free_port(3306, &mut reserved),
                php_fastcgi_port: claim_free_port(9000, &mut reserved),
                dns_port: claim_free_port(default_dns, &mut reserved),
            }
        }
        _ => return Err(format!("Unknown port preset: {preset}")),
    };

    update_ports(payload, state).await
}

#[tauri::command]
async fn open_in_browser(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL is empty.".into());
    }
    open::that(trimmed).map_err(|e| format!("failed to open browser: {e}"))
}

#[tauri::command]
async fn open_project_terminal(
    project_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (active_php, mysql_port, php_home) = {
        let settings = state.settings.lock().await;
        (
            settings.active_php_version.clone(),
            settings.mysql_port,
            settings.php_home(&state.app_root, &settings.active_php_version),
        )
    };
    state
        .cli_env
        .open_project_terminal_from(
            Path::new(&project_path),
            &php_home,
            &active_php,
            mysql_port,
        )
        .map_err(|e| e.to_string())
}

// ============================================================================
// Config Editor (3-layer hybrid: visual + custom raw + nginx -t rollback)
// ============================================================================

#[tauri::command]
async fn get_php_extensions_list(
    state: State<'_, AppState>,
) -> Result<Vec<PhpExtensionInfo>, String> {
    let settings = state.settings.lock().await;
    state
        .config_editor
        .scan_available_php_extensions(
            &settings.active_php_version,
            &settings.php_settings.enabled_extensions,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_visual_config(
    target: ConfigTarget,
    php_settings: Option<PhpVisualSettings>,
    mysql_settings: Option<MySqlVisualSettings>,
    nginx_settings: Option<NginxVisualSettings>,
    state: State<'_, AppState>,
) -> Result<PersistentSettings, String> {
    let updated = {
        let mut settings = state.settings.lock().await;
        if let Some(php) = php_settings {
            settings.php_settings = php;
        }
        if let Some(mysql) = mysql_settings {
            settings.mysql_settings = mysql;
        }
        if let Some(nginx) = nginx_settings {
            settings.nginx_settings = nginx;
        }
        settings.save(&state.app_root)?;
        settings.clone()
    };

    let sandbox_cfg = state.build_sandbox_config().await;
    render_all_configs(&state.app_root, &sandbox_cfg).map_err(|e| e.to_string())?;
    state.reload_single_service(target).await?;

    append_praews_log(
        &state.app_root,
        &format!("Visual config saved for {:?}", target),
    );
    Ok(updated)
}

#[tauri::command]
async fn get_custom_raw_config(
    target: ConfigTarget,
    state: State<'_, AppState>,
) -> Result<String, String> {
    Ok(state.config_editor.read_custom_raw(target))
}

#[tauri::command]
async fn save_custom_raw_config(
    target: ConfigTarget,
    raw_content: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let backup_content = state.config_editor.read_custom_raw(target);

    state
        .config_editor
        .save_custom_raw(target, &raw_content)
        .map_err(|e| e.to_string())?;

    let sandbox_cfg = state.build_sandbox_config().await;
    render_all_configs(&state.app_root, &sandbox_cfg).map_err(|e| e.to_string())?;

    if target == ConfigTarget::Nginx {
        if let Err(validation_err) = state.config_editor.validate_nginx_config().await {
            let _ = state.config_editor.save_custom_raw(target, &backup_content);
            let fallback_cfg = state.build_sandbox_config().await;
            let _ = render_all_configs(&state.app_root, &fallback_cfg);
            return Err(validation_err.to_string());
        }
    }

    state.reload_single_service(target).await?;
    append_praews_log(
        &state.app_root,
        &format!("Custom raw config saved for {:?}", target),
    );
    Ok(())
}

#[tauri::command]
async fn reset_service_config(
    target: ConfigTarget,
    state: State<'_, AppState>,
) -> Result<PersistentSettings, String> {
    let updated = {
        let mut settings = state.settings.lock().await;
        match target {
            ConfigTarget::Php => settings.php_settings = PhpVisualSettings::default(),
            ConfigTarget::MySql => settings.mysql_settings = MySqlVisualSettings::default(),
            ConfigTarget::Nginx => settings.nginx_settings = NginxVisualSettings::default(),
        }
        settings.save(&state.app_root)?;
        settings.clone()
    };

    state
        .config_editor
        .save_custom_raw(target, "")
        .map_err(|e| e.to_string())?;

    let sandbox_cfg = state.build_sandbox_config().await;
    render_all_configs(&state.app_root, &sandbox_cfg).map_err(|e| e.to_string())?;
    state.reload_single_service(target).await?;

    append_praews_log(
        &state.app_root,
        &format!("Config reset to factory defaults for {:?}", target),
    );
    Ok(updated)
}

#[tauri::command]
async fn open_rendered_config_in_os(
    target: ConfigTarget,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .config_editor
        .open_in_external_editor(target)
        .map_err(|e| e.to_string())
}

/// Brand mark used for Dock (macOS) when the bundled default icon is missing.
const APP_ICON_PNG: &[u8] = include_bytes!("../icons/icon.png");
/// Menu-bar tray template: mark only on transparent (no filled square).
const TRAY_TEMPLATE_PNG: &[u8] = include_bytes!("../icons/tray-template.png");

fn load_app_icon() -> Option<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(APP_ICON_PNG).ok()
}

fn load_tray_icon() -> Option<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(TRAY_TEMPLATE_PNG).ok()
}

/// macOS Dock uses the .app bundle icon — `cargo run` has none, so set it at runtime.
#[cfg(target_os = "macos")]
fn apply_macos_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(APP_ICON_PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    // SAFETY: called on the AppKit main thread with a valid NSImage from PNG bytes.
    unsafe {
        NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
    }
}

#[cfg(not(target_os = "macos"))]
fn apply_macos_dock_icon() {}

const TRAY_ID: &str = "main";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StackKind {
    Stopped,
    Starting,
    Partial { running: usize, total: usize },
    Running,
    Degraded,
}

impl StackKind {
    fn from_statuses(statuses: &HashMap<ServiceType, ServiceStatus>) -> Self {
        const CORE: [ServiceType; 3] = [ServiceType::Nginx, ServiceType::Php, ServiceType::MySql];
        let total = CORE.len();
        let mut running = 0usize;
        let mut starting = 0usize;
        let mut errored = 0usize;
        for service in CORE {
            match statuses.get(&service) {
                Some(ServiceStatus::Running { .. }) => running += 1,
                Some(ServiceStatus::Starting) => starting += 1,
                Some(ServiceStatus::Error(_)) => errored += 1,
                _ => {}
            }
        }
        if running == total {
            Self::Running
        } else if starting > 0 && running == 0 {
            Self::Starting
        } else if running > 0 {
            Self::Partial { running, total }
        } else if errored > 0 {
            Self::Degraded
        } else {
            Self::Stopped
        }
    }

    fn menu_label(self) -> String {
        match self {
            Self::Stopped => "Stack · Stopped".into(),
            Self::Starting => "Stack · Starting…".into(),
            Self::Partial { running, total } => format!("Stack · Partial ({running}/{total})"),
            Self::Running => "Stack · Running".into(),
            Self::Degraded => "Stack · Error".into(),
        }
    }

    fn tooltip(self) -> String {
        format!("praews — {}", self.short_label())
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Partial { .. } => "Partial",
            Self::Running => "Running",
            Self::Degraded => "Error",
        }
    }

    /// Compact macOS menu-bar title next to the tray icon (singular status).
    fn macos_title(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Partial { .. } | Self::Starting => "◐",
            Self::Degraded => "!",
            Self::Stopped => "",
        }
    }
}

fn build_tray_menu(
    app: &impl Manager<tauri::Wry>,
    status_label: &str,
) -> tauri::Result<Menu<tauri::Wry>> {
    let status_item = MenuItem::with_id(app, "stack_status", status_label, false, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show", "Show Dashboard", true, None::<&str>)?;
    let start_all = MenuItem::with_id(app, "start_all", "Start All Services", true, None::<&str>)?;
    let stop_all = MenuItem::with_id(app, "stop_all", "Stop All Services", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit & Stop Servers", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &status_item,
            &separator,
            &show_item,
            &separator,
            &start_all,
            &stop_all,
            &separator,
            &quit_item,
        ],
    )
}

async fn refresh_tray_status(app: &AppHandle) {
    use std::sync::Mutex as StdMutex;
    static LAST_KIND: StdMutex<Option<StackKind>> = StdMutex::new(None);

    let kind = {
        let state = app.state::<AppState>();
        let statuses = state.orchestrator.get_all_statuses().await;
        StackKind::from_statuses(&statuses)
    };

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    let _ = tray.set_tooltip(Some(kind.tooltip()));
    let _ = tray.set_title(Some(kind.macos_title()));

    let changed = {
        let mut last = LAST_KIND.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *last != Some(kind);
        *last = Some(kind);
        changed
    };

    // Rebuild menu only when the singular status changes (avoids closing an open tray menu).
    if changed {
        if let Ok(menu) = build_tray_menu(app, &kind.menu_label()) {
            let _ = tray.set_menu(Some(menu));
        }
        let _ = app.emit(
            "stack-status",
            serde_json::json!({
                "kind": kind.short_label().to_lowercase(),
                "label": kind.menu_label(),
            }),
        );
    }
}

fn setup_system_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let tray_menu = build_tray_menu(app, "Stack · Stopped")?;

    // Use a silhouette mark (transparent bg). The full app icon becomes a solid white
    // blob under macOS `icon_as_template` because its filled square is all opaque.
    let tray_icon = load_tray_icon()
        .or_else(load_app_icon)
        .ok_or("praews tray icon missing (icons/tray-template.png)")?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon)
        .menu(&tray_menu)
        .tooltip("praews — Stopped")
        .show_menu_on_left_click(false)
        .on_menu_event(|app_handle, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "start_all" => {
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    let _ = start_all_services(state).await;
                    let _ = handle.emit("services-changed", ());
                    refresh_tray_status(&handle).await;
                });
            }
            "stop_all" => {
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    let _ = stop_all_services(state).await;
                    let _ = handle.emit("services-changed", ());
                    refresh_tray_status(&handle).await;
                });
            }
            "quit" => {
                app_handle.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app_handle = tray.app_handle();
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        });

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true).title("");
    }

    let _tray = builder.build(app)?;

    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        // Initial paint, then keep the singular tray status in sync.
        refresh_tray_status(&handle).await;
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            ticker.tick().await;
            refresh_tray_status(&handle).await;
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    let app_root = resolve_portable_app_root();
    let _ = init_sandbox(&app_root);
    let initial_settings = PersistentSettings::load(&app_root);
    let dns_port = initial_settings.dns_port;

    let binary_manager = BinaryManager::new(&app_root).expect("binary manager");
    let app_state = AppState {
        app_root: app_root.clone(),
        settings: Mutex::new(initial_settings),
        orchestrator: Arc::new(ProcessOrchestrator::new(&app_root)),
        dns_server: Mutex::new(LocalDnsServer::new(dns_port)),
        binary_manager: Arc::new(binary_manager),
        ssl_manager: Arc::new(SslManager::new(&app_root)),
        cli_env: Arc::new(CliEnvManager::new(&app_root)),
        config_editor: Arc::new(ConfigEditorManager::new(&app_root)),
    };

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .manage(app_state);

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    let app = builder
        .setup(|app| {
            apply_macos_dock_icon();
            setup_system_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            get_setup_status,
            run_first_setup,
            get_service_statuses,
            start_all_services,
            start_service,
            stop_service,
            stop_all_services,
            ensure_dns_resolver,
            list_php_catalog,
            add_php_version,
            switch_php_version,
            register_custom_php_path,
            clear_custom_php_path,
            remove_php_version,
            download_and_install_binary,
            install_manifest_component,
            create_virtual_host,
            list_htdocs_folders,
            trust_local_root_ca,
            check_ports_availability,
            update_ports,
            apply_port_preset,
            open_project_terminal,
            open_in_browser,
            read_recent_logs,
            clear_logs,
            change_app_root,
            reset_app_root,
            update_htdocs_root,
            verify_portable,
            get_php_extensions_list,
            save_visual_config,
            get_custom_raw_config,
            save_custom_raw_config,
            reset_service_config,
            open_rendered_config_in_os,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Tauri app");

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            let state = app_handle.state::<AppState>();
            let orchestrator = state.orchestrator.clone();
            tauri::async_runtime::block_on(async move {
                let _ = orchestrator.stop_all().await;
            });
        }
    });
}

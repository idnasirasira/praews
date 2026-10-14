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
    add_site, get_local_lan_ip, init_sandbox, render_all_configs, render_all_vhosts,
    verify_portable_layout, BinaryManager, CliEnvManager, ComponentSpec, LocalDnsServer,
    ProcessOrchestrator, SandboxConfig, ServiceStatus, ServiceType, SslManager,
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
}

pub struct AppState {
    pub app_root: PathBuf,
    pub settings: Mutex<PersistentSettings>,
    pub orchestrator: Arc<ProcessOrchestrator>,
    pub dns_server: Mutex<LocalDnsServer>,
    pub binary_manager: Arc<BinaryManager>,
    pub ssl_manager: Arc<SslManager>,
    pub cli_env: Arc<CliEnvManager>,
}

fn resolve_portable_app_root() -> PathBuf {
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

#[tauri::command]
async fn get_app_info(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().await.clone();
    let lan_ip = get_local_lan_ip();
    let mobile_url = format_host_url("http", &format!("praews.{lan_ip}.sslip.io"), settings.http_port);
    let root_ca_url = format!(
        "{}/rootCA.crt",
        format_host_url("http", &lan_ip.to_string(), settings.http_port)
    );
    let php_versions = list_available_php_versions(&state);
    Ok(serde_json::json!({
        "app_root": state.app_root.to_string_lossy(),
        "lan_ip": lan_ip.to_string(),
        "mobile_url": mobile_url,
        "root_ca_url": root_ca_url,
        "dns_hint": format!(
            "Phone Wi-Fi DNS → {} (port {}). Or open {}",
            lan_ip, settings.dns_port, mobile_url
        ),
        "available_php_versions": php_versions,
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

#[tauri::command]
async fn read_recent_logs(
    state: State<'_, AppState>,
    lines: Option<usize>,
) -> Result<String, String> {
    let n = lines.unwrap_or(80);
    let log_dir = state.app_root.join("logs");
    let mut out = String::new();
    for name in LOG_FILE_NAMES {
        let path = log_dir.join(name);
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        let tail: Vec<&str> = content
            .lines()
            .rev()
            .take(n / LOG_FILE_NAMES.len().max(1))
            .collect();
        if tail.is_empty() {
            continue;
        }
        out.push_str(&format!("===== {name} =====\n"));
        for line in tail.into_iter().rev() {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("(no log files yet)");
    }
    Ok(out)
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

fn list_available_php_versions(state: &AppState) -> Vec<String> {
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

#[tauri::command]
async fn start_all_services(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    let root = state.app_root.clone();

    let fail = |msg: String| -> Result<(), String> {
        append_praews_log(&root, &format!("ERROR: {msg}"));
        Err(msg)
    };

    append_praews_log(&root, "Start All requested");

    init_sandbox(&state.app_root).map_err(|e| {
        let msg = e.to_string();
        append_praews_log(&root, &format!("ERROR: {msg}"));
        msg
    })?;

    let missing = state
        .binary_manager
        .missing_runtime_binaries(&settings.active_php_version);
    if !missing.is_empty() {
        let msg = format!(
            "Binaries belum terpasang:\n- {}\nJalankan first-run setup (atau Install Nginx / PHP / MariaDB), baru Start.",
            missing.join("\n- ")
        );
        return fail(msg);
    }

    state
        .binary_manager
        .ensure_cacert_bundle()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            append_praews_log(&root, &format!("ERROR: {msg}"));
            msg
        })?;

    let config = SandboxConfig::new(&state.app_root, &settings.active_php_version)
        .with_ports(
            settings.http_port,
            settings.php_fastcgi_port,
            settings.mysql_port,
        )
        .with_https_port(settings.https_port);
    render_all_configs(&state.app_root, &config).map_err(|e| {
        let msg = e.to_string();
        append_praews_log(&root, &format!("ERROR: {msg}"));
        msg
    })?;

    state
        .binary_manager
        .initialize_mysql_datadir()
        .await
        .map_err(|e| {
            let msg = format!("MySQL init: {e}");
            append_praews_log(&root, &format!("ERROR: {msg}"));
            msg
        })?;

    state
        .orchestrator
        .start_mysql(settings.mysql_port)
        .await
        .map_err(|e| {
            let msg = format!("MySQL: {e}");
            append_praews_log(&root, &format!("ERROR: {msg}"));
            msg
        })?;
    state
        .orchestrator
        .start_php(&settings.active_php_version, settings.php_fastcgi_port)
        .await
        .map_err(|e| {
            let msg = format!("PHP: {e}");
            append_praews_log(&root, &format!("ERROR: {msg}"));
            msg
        })?;
    state
        .orchestrator
        .start_nginx(settings.http_port)
        .await
        .map_err(|e| {
            let msg = format!("Nginx: {e}");
            append_praews_log(&root, &format!("ERROR: {msg}"));
            msg
        })?;

    let mut dns = state.dns_server.lock().await;
    match dns.start().await {
        Ok(()) => append_praews_log(&root, "DNS started"),
        Err(e) => append_praews_log(&root, &format!("DNS warning: {e}")),
    }

    append_praews_log(&root, "Start All completed");
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
async fn switch_php_version(version: String, state: State<'_, AppState>) -> Result<(), String> {
    let php_dir = state.app_root.join("bin/php").join(&version);
    if !php_dir.exists() {
        return Err(format!(
            "PHP {version} is not installed under bin/php/{version}"
        ));
    }

    let (http_port, php_port, mysql_port, https_port) = {
        let mut settings = state.settings.lock().await;
        settings.active_php_version = version.clone();
        settings.save(&state.app_root)?;
        (
            settings.http_port,
            settings.php_fastcgi_port,
            settings.mysql_port,
            settings.https_port,
        )
    };

    let config = SandboxConfig::new(&state.app_root, &version)
        .with_ports(http_port, php_port, mysql_port)
        .with_https_port(https_port);
    render_all_configs(&state.app_root, &config).map_err(|e| e.to_string())?;

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
            .start_php(&version, php_port)
            .await
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

    let sandbox_cfg = SandboxConfig::new(&state.app_root, &updated_settings.active_php_version)
        .with_ports(
            updated_settings.http_port,
            updated_settings.php_fastcgi_port,
            updated_settings.mysql_port,
        )
        .with_https_port(updated_settings.https_port);

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
        "mamp_safe" => PortConfigPayload {
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
async fn open_project_terminal(
    project_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (active_php, mysql_port) = {
        let settings = state.settings.lock().await;
        (settings.active_php_version.clone(), settings.mysql_port)
    };
    state
        .cli_env
        .open_project_terminal(Path::new(&project_path), &active_php, mysql_port)
        .map_err(|e| e.to_string())
}

fn setup_system_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(app, "show", "Show Dashboard", true, None::<&str>)?;
    let start_all = MenuItem::with_id(app, "start_all", "Start All Services", true, None::<&str>)?;
    let stop_all = MenuItem::with_id(app, "stop_all", "Stop All Services", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit & Stop Servers", true, None::<&str>)?;
    let tray_menu = Menu::with_items(
        app,
        &[
            &show_item,
            &separator,
            &start_all,
            &stop_all,
            &separator,
            &quit_item,
        ],
    )?;

    let _tray = TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&tray_menu)
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
                });
            }
            "stop_all" => {
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    let _ = stop_all_services(state).await;
                    let _ = handle.emit("services-changed", ());
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
        })
        .build(app)?;

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
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .setup(|app| {
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
            stop_all_services,
            switch_php_version,
            download_and_install_binary,
            install_manifest_component,
            create_virtual_host,
            trust_local_root_ca,
            check_ports_availability,
            update_ports,
            apply_port_preset,
            open_project_terminal,
            read_recent_logs,
            clear_logs,
            verify_portable,
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

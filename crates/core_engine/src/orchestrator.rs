use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServiceType {
    Nginx,
    Php,
    MySql,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running { pid: u32, port: u16 },
    Error(String),
}

struct ManagedProcess {
    child: Child,
    pid: u32,
    port: u16,
}

/// Starts, monitors, and gracefully stops Nginx / PHP / MySQL child processes.
pub struct ProcessOrchestrator {
    app_root: PathBuf,
    processes: Arc<Mutex<HashMap<ServiceType, ManagedProcess>>>,
}

impl ProcessOrchestrator {
    pub fn new(app_root: impl Into<PathBuf>) -> Self {
        Self {
            app_root: app_root.into(),
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn app_root(&self) -> &Path {
        &self.app_root
    }

    /// Returns true when `127.0.0.1:port` can be bound.
    pub fn is_port_available(port: u16) -> bool {
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }

    /// Kill leftover Nginx/PHP/MySQL from a previous force-quit, then require a free port.
    async fn ensure_port_ready(&self, service: ServiceType, port: u16) -> Result<()> {
        self.reclaim_orphans_for(service, port).await;
        if Self::is_port_available(port) {
            return Ok(());
        }
        // One more pass after a short wait — shutdown can be slow (MariaDB).
        sleep(Duration::from_millis(400)).await;
        self.reclaim_orphans_for(service, port).await;
        if Self::is_port_available(port) {
            Ok(())
        } else {
            bail!("port {port} is already in use")
        }
    }

    /// Force-quit leaves children alive; reclaim via pidfile and/or listeners under app_root.
    async fn reclaim_orphans_for(&self, service: ServiceType, port: u16) {
        if let Some((pid, saved_port)) = self.read_pid_file(service) {
            let stop_port = if port != 0 { port } else { saved_port };
            if process_alive(pid) {
                tracing::warn!(?service, pid, "reclaiming stale pidfile process");
                if service == ServiceType::MySql && stop_port != 0 {
                    let _ = self.graceful_stop_mysql(stop_port).await;
                    sleep(Duration::from_millis(500)).await;
                }
                if service == ServiceType::Nginx {
                    let _ = self.graceful_stop_nginx().await;
                    sleep(Duration::from_millis(200)).await;
                }
                Self::force_kill_process_tree(pid).await;
            }
            self.clear_pid_file(service);
        }

        // php-fpm workers don't keep the conf path in argv — match master by our conf.
        if service == ServiceType::Php {
            self.reclaim_php_fpm_by_conf().await;
        }

        if port == 0 || Self::is_port_available(port) {
            return;
        }

        let pids = listener_pids_on_port(port).await;
        for pid in pids {
            let target = reclaim_root_pid(pid, &self.app_root).await;
            if let Some(root_pid) = target {
                tracing::warn!(
                    ?service,
                    pid = root_pid,
                    port,
                    "reclaiming orphan listener under app_root"
                );
                if service == ServiceType::MySql {
                    let _ = self.graceful_stop_mysql(port).await;
                    sleep(Duration::from_millis(500)).await;
                }
                Self::force_kill_process_tree(root_pid).await;
                self.clear_pid_file(service);
                if Self::is_port_available(port) {
                    break;
                }
            }
        }
    }

    async fn reclaim_php_fpm_by_conf(&self) {
        let conf = path_for_config(&self.app_root.join("conf/php-fpm.conf"));
        if conf.is_empty() {
            return;
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Master process title: php-fpm: master process (/path/php-fpm.conf)
            let _ = Command::new("pkill")
                .args(["-TERM", "-f", &format!("php-fpm: master process ({conf})")])
                .output()
                .await;
            sleep(Duration::from_millis(250)).await;
            let _ = Command::new("pkill")
                .args(["-KILL", "-f", &format!("php-fpm: master process ({conf})")])
                .output()
                .await;
        }
        #[cfg(target_os = "windows")]
        {
            let _ = conf;
        }
    }

    fn pid_file_path(&self, service: ServiceType) -> PathBuf {
        let name = match service {
            ServiceType::Nginx => "nginx.pid",
            ServiceType::Php => "php.pid",
            ServiceType::MySql => "mysql.pid",
        };
        self.app_root.join("run").join(name)
    }

    fn write_pid_file(&self, service: ServiceType, pid: u32, port: u16) {
        let path = self.pid_file_path(service);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, format!("{pid} {port}\n"));
    }

    fn read_pid_file(&self, service: ServiceType) -> Option<(u32, u16)> {
        let raw = std::fs::read_to_string(self.pid_file_path(service)).ok()?;
        let mut parts = raw.split_whitespace();
        let pid = parts.next()?.parse().ok()?;
        let port = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        Some((pid, port))
    }

    fn clear_pid_file(&self, service: ServiceType) {
        let _ = std::fs::remove_file(self.pid_file_path(service));
    }

    pub async fn start_nginx(&self, http_port: u16) -> Result<u32> {
        self.ensure_port_ready(ServiceType::Nginx, http_port).await?;

        let bin_name = if cfg!(target_os = "windows") {
            "nginx.exe"
        } else {
            "nginx"
        };
        let nginx_bin = self.app_root.join("bin/nginx").join(bin_name);
        let nginx_prefix = self.app_root.join("bin/nginx");
        let nginx_conf = self.app_root.join("conf/nginx.conf");

        ensure_binary_exists(&nginx_bin)?;

        let mut cmd = Command::new(&nginx_bin);
        cmd.arg("-p")
            .arg(path_for_config(&nginx_prefix))
            .arg("-c")
            .arg(path_for_config(&nginx_conf));

        self.spawn_and_track(ServiceType::Nginx, cmd, http_port, "nginx_process.log")
            .await
    }

    pub async fn start_php(&self, version: &str, fastcgi_port: u16) -> Result<u32> {
        let php_dir = self.app_root.join("bin/php").join(version);
        self.start_php_at(&php_dir, fastcgi_port).await
    }

    /// Start PHP-FPM / php-cgi from an explicit home (sandbox or linked custom path).
    pub async fn start_php_at(&self, php_dir: &Path, fastcgi_port: u16) -> Result<u32> {
        self.ensure_port_ready(ServiceType::Php, fastcgi_port)
            .await
            .with_context(|| format!("FastCGI port {fastcgi_port}"))?;

        let php_ini = self.app_root.join("conf/php.ini");
        let bind_addr = format!("127.0.0.1:{fastcgi_port}");

        let mut cmd = if cfg!(target_os = "windows") {
            let bin = php_dir.join("php-cgi.exe");
            ensure_binary_exists(&bin)?;
            let mut c = Command::new(bin);
            c.arg("-b").arg(&bind_addr).arg("-c").arg(path_for_config(&php_ini));
            c.env("PHP_FCGI_MAX_REQUESTS", "0");
            c
        } else {
            let bin = find_php_fpm(php_dir).ok_or_else(|| {
                anyhow::anyhow!(
                    "php-fpm not found under {:?}. Install or link PHP first.",
                    php_dir
                )
            })?;
            let fpm_conf = self.app_root.join("conf/php-fpm.conf");
            if !fpm_conf.exists() {
                bail!(
                    "missing {} — run render/config first so PHP-FPM knows listen=:{}",
                    fpm_conf.display(),
                    fastcgi_port
                );
            }
            let mut c = Command::new(bin);
            // -F foreground; -y fpm pool config (listen port); -c php.ini
            c.arg("-F")
                .arg("-O")
                .arg("-y")
                .arg(path_for_config(&fpm_conf))
                .arg("-c")
                .arg(path_for_config(&php_ini));
            let _ = bind_addr; // listen address comes from php-fpm.conf
            c
        };

        crate::php_runtime::apply_macos_php_library_env(&mut cmd, php_dir);

        self.spawn_and_track(ServiceType::Php, cmd, fastcgi_port, "php_process.log")
            .await
    }

    pub async fn start_mysql(&self, mysql_port: u16) -> Result<u32> {
        self.ensure_port_ready(ServiceType::MySql, mysql_port).await?;

        let mysql_bin_dir = self.app_root.join("bin/mysql/bin");
        // Drop legacy mysqld→mariadbd alias so argv0 is not the deprecated name.
        let _ = std::fs::remove_file(mysql_bin_dir.join("mysqld"));
        let _ = std::fs::remove_file(mysql_bin_dir.join("mysqld.exe"));

        let mysqld_bin = find_mysqld(&mysql_bin_dir).ok_or_else(|| {
            anyhow::anyhow!(
                "mariadbd/mysqld not found under {:?}. Install MariaDB first.",
                mysql_bin_dir
            )
        })?;
        let my_cnf = self.app_root.join("conf/my.cnf");

        let mut cmd = Command::new(&mysqld_bin);
        // `--defaults-file` MUST be the first argument for mariadbd/mysqld.
        cmd.arg(format!("--defaults-file={}", path_for_config(&my_cnf)))
            .arg("--console");

        self.spawn_and_track(ServiceType::MySql, cmd, mysql_port, "mysql_process.log")
            .await
    }

    async fn spawn_and_track(
        &self,
        service: ServiceType,
        mut cmd: Command,
        port: u16,
        log_filename: &str,
    ) -> Result<u32> {
        let mut map = self.processes.lock().await;
        if map.contains_key(&service) {
            bail!("{service:?} is already running");
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn binary for {service:?}"))?;

        let pid = child
            .id()
            .context("failed to read PID from child process")?;

        let log_path = self.app_root.join("logs").join(log_filename);
        if let Some(stdout) = child.stdout.take() {
            Self::stream_output_to_file(stdout, log_path.clone(), "STDOUT");
        }
        if let Some(stderr) = child.stderr.take() {
            Self::stream_output_to_file(stderr, log_path, "STDERR");
        }

        map.insert(service, ManagedProcess { child, pid, port });
        self.write_pid_file(service, pid, port);
        tracing::info!(?service, pid, port, "service started");
        Ok(pid)
    }

    pub async fn stop_service(&self, service: ServiceType) -> Result<()> {
        let mut map = self.processes.lock().await;
        if let Some(mut managed) = map.remove(&service) {
            let pid = managed.pid;

            if service == ServiceType::Nginx {
                let _ = self.graceful_stop_nginx().await;
                sleep(Duration::from_millis(300)).await;
            }

            if service == ServiceType::MySql {
                let _ = self.graceful_stop_mysql(managed.port).await;
                sleep(Duration::from_millis(800)).await;
            }

            Self::force_kill_process_tree(pid).await;
            let _ = managed.child.wait().await;
            self.clear_pid_file(service);
            tracing::info!(?service, pid, "service stopped");
        } else {
            // Not tracked in this session (e.g. after force-quit / relaunch).
            self.reclaim_orphans_for(service, 0).await;
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<()> {
        let _ = self.stop_service(ServiceType::Nginx).await;
        let _ = self.stop_service(ServiceType::Php).await;
        let _ = self.stop_service(ServiceType::MySql).await;
        Ok(())
    }

    pub async fn get_all_statuses(&self) -> HashMap<ServiceType, ServiceStatus> {
        let mut map = self.processes.lock().await;
        let mut statuses = HashMap::new();

        for service in [ServiceType::Nginx, ServiceType::Php, ServiceType::MySql] {
            if let Some(managed) = map.get_mut(&service) {
                match managed.child.try_wait() {
                    Ok(Some(exit_status)) => {
                        statuses.insert(
                            service,
                            ServiceStatus::Error(format!("exited unexpectedly ({exit_status})")),
                        );
                        map.remove(&service);
                    }
                    Ok(None) => {
                        statuses.insert(
                            service,
                            ServiceStatus::Running {
                                pid: managed.pid,
                                port: managed.port,
                            },
                        );
                    }
                    Err(e) => {
                        statuses.insert(service, ServiceStatus::Error(e.to_string()));
                    }
                }
            } else {
                statuses.insert(service, ServiceStatus::Stopped);
            }
        }

        statuses
    }

    async fn graceful_stop_nginx(&self) -> Result<()> {
        let bin_name = if cfg!(target_os = "windows") {
            "nginx.exe"
        } else {
            "nginx"
        };
        let nginx_bin = self.app_root.join("bin/nginx").join(bin_name);
        let nginx_prefix = self.app_root.join("bin/nginx");
        let nginx_conf = self.app_root.join("conf/nginx.conf");

        let mut cmd = Command::new(nginx_bin);
        cmd.arg("-p")
            .arg(path_for_config(&nginx_prefix))
            .arg("-c")
            .arg(path_for_config(&nginx_conf))
            .arg("-s")
            .arg("quit");

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let _ = cmd.output().await;
        Ok(())
    }

    async fn graceful_stop_mysql(&self, port: u16) -> Result<()> {
        let bin_name = if cfg!(target_os = "windows") {
            "mysqladmin.exe"
        } else {
            "mysqladmin"
        };
        let admin_bin = self.app_root.join("bin/mysql/bin").join(bin_name);

        let mut cmd = Command::new(admin_bin);
        cmd.arg("-u")
            .arg("root")
            .arg("-h")
            .arg("127.0.0.1")
            .arg("-P")
            .arg(port.to_string())
            .arg("shutdown");

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let _ = cmd.output().await;
        Ok(())
    }

    async fn force_kill_process_tree(pid: u32) {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let mut cmd = Command::new("taskkill");
            cmd.args(["/F", "/T", "/PID", &pid.to_string()]);
            cmd.creation_flags(CREATE_NO_WINDOW);
            let _ = cmd.output().await;
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = Command::new("pkill")
                .args(["-TERM", "-P", &pid.to_string()])
                .output()
                .await;
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output()
                .await;
            sleep(Duration::from_millis(200)).await;
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .output()
                .await;
        }
    }

    fn stream_output_to_file<R>(reader: R, log_path: PathBuf, prefix: &'static str)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = writeln!(file, "[{prefix}] {line}");
                }
            }
        });
    }
}

fn path_for_config(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        // OpenProcess would be better; tasklist is fine for MVP.
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // signal 0: existence check
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

async fn listener_pids_on_port(port: u16) -> Vec<u32> {
    #[cfg(target_os = "windows")]
    {
        let mut pids = Vec::new();
        let Ok(output) = Command::new("netstat").args(["-ano"]).output().await else {
            return pids;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let needle = format!(":{port}");
        for line in stdout.lines() {
            let line = line.trim();
            if !line.contains(&needle) || !line.contains("LISTENING") {
                continue;
            }
            if let Some(pid_s) = line.split_whitespace().last() {
                if let Ok(pid) = pid_s.parse::<u32>() {
                    if !pids.contains(&pid) {
                        pids.push(pid);
                    }
                }
            }
        }
        pids
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Ok(output) = Command::new("lsof")
            .args([
                "-nP",
                &format!("-iTCP:{port}"),
                "-sTCP:LISTEN",
                "-t",
            ])
            .output()
            .await
        else {
            return Vec::new();
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut pids = Vec::new();
        for line in stdout.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        pids
    }
}

/// If `pid` (or its parent) is a praews-owned process, return the root pid to kill.
async fn reclaim_root_pid(pid: u32, app_root: &Path) -> Option<u32> {
    if process_belongs_to_app_root(pid, app_root).await {
        return Some(pid);
    }
    // php-fpm workers: argv is just "php-fpm: pool www" — check parent master.
    if let Some(ppid) = parent_pid(pid).await {
        if ppid > 1 && process_belongs_to_app_root(ppid, app_root).await {
            return Some(ppid);
        }
    }
    None
}

async fn parent_pid(pid: u32) -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("ProcessId={pid}"),
                "get",
                "ParentProcessId",
                "/value",
            ])
            .output()
            .await
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("ParentProcessId=") {
                return v.trim().parse().ok();
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "ppid="])
            .output()
            .await
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()
    }
}

async fn process_belongs_to_app_root(pid: u32, app_root: &Path) -> bool {
    let root = app_root.to_string_lossy().replace('\\', "/");

    #[cfg(target_os = "windows")]
    {
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("ProcessId={pid}"),
                "get",
                "ExecutablePath",
                "/value",
            ])
            .output()
            .await;
        let Ok(output) = output else {
            return false;
        };
        let text = String::from_utf8_lossy(&output.stdout).replace('\\', "/");
        text.to_lowercase().contains(&root.to_lowercase())
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Prefer argv (covers Homebrew-linked PHP under /opt while still using our conf).
        let args = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "args="])
            .output()
            .await
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if args.replace('\\', "/").contains(root.as_str()) {
            return true;
        }
        // Fallback: executable path via lsof
        let output = Command::new("lsof")
            .args(["-p", &pid.to_string(), "-Fn"])
            .output()
            .await;
        let Ok(output) = output else {
            return false;
        };
        String::from_utf8_lossy(&output.stdout)
            .replace('\\', "/")
            .contains(root.as_str())
    }
}

fn ensure_binary_exists(path: &Path) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        bail!(
            "binary not found at {:?}. Use Install buttons (Nginx / PHP / MariaDB) first.",
            path
        )
    }
}

fn find_php_fpm(php_dir: &Path) -> Option<PathBuf> {
    [
        php_dir.join("sbin/php-fpm"),
        php_dir.join("php-fpm"),
        php_dir.join("bin/php-fpm"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

fn find_mysqld(bin_dir: &Path) -> Option<PathBuf> {
    // Prefer mariadbd: calling via the `mysqld` compatibility name is deprecated.
    [
        bin_dir.join("mariadbd"),
        bin_dir.join("mariadbd.exe"),
        bin_dir.join("mysqld"),
        bin_dir.join("mysqld.exe"),
    ]
    .into_iter()
    .find(|p| p.exists() && !is_deprecated_mysqld_alias(p))
}

fn is_deprecated_mysqld_alias(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name != "mysqld" && name != "mysqld.exe" {
        return false;
    }
    // Skip symlink/alias named mysqld when the real mariadbd sits beside it.
    let sibling = path.with_file_name(if name.ends_with(".exe") {
        "mariadbd.exe"
    } else {
        "mariadbd"
    });
    sibling.exists()
}

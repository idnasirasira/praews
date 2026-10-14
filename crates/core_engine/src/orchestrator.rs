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

    pub async fn start_nginx(&self, http_port: u16) -> Result<u32> {
        if !Self::is_port_available(http_port) {
            bail!("port {http_port} is already in use");
        }

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
        if !Self::is_port_available(fastcgi_port) {
            bail!("FastCGI port {fastcgi_port} is already in use");
        }

        let php_dir = self.app_root.join("bin/php").join(version);
        let php_ini = self.app_root.join("conf/php.ini");
        let bind_addr = format!("127.0.0.1:{fastcgi_port}");

        let cmd = if cfg!(target_os = "windows") {
            let bin = php_dir.join("php-cgi.exe");
            ensure_binary_exists(&bin)?;
            let mut c = Command::new(bin);
            c.arg("-b").arg(&bind_addr).arg("-c").arg(path_for_config(&php_ini));
            c.env("PHP_FCGI_MAX_REQUESTS", "0");
            c
        } else {
            let bin = find_php_fpm(&php_dir).ok_or_else(|| {
                anyhow::anyhow!(
                    "php-fpm not found under {:?}. Install PHP first.",
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

        self.spawn_and_track(ServiceType::Php, cmd, fastcgi_port, "php_process.log")
            .await
    }

    pub async fn start_mysql(&self, mysql_port: u16) -> Result<u32> {
        if !Self::is_port_available(mysql_port) {
            bail!("MySQL port {mysql_port} is already in use");
        }

        let mysql_bin_dir = self.app_root.join("bin/mysql/bin");
        let mysqld_bin = find_mysqld(&mysql_bin_dir).ok_or_else(|| {
            anyhow::anyhow!(
                "mysqld/mariadbd not found under {:?}. Install MariaDB first.",
                mysql_bin_dir
            )
        })?;
        let my_cnf = self.app_root.join("conf/my.cnf");

        let mut cmd = Command::new(&mysqld_bin);
        // `--defaults-file` MUST be the first argument for mysqld.
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
            tracing::info!(?service, pid, "service stopped");
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
    [
        bin_dir.join("mysqld.exe"),
        bin_dir.join("mariadbd.exe"),
        bin_dir.join("mysqld"),
        bin_dir.join("mariadbd"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

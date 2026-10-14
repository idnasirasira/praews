use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PhpVisualSettings {
    pub memory_limit: String,
    pub upload_max_filesize: String,
    pub post_max_size: String,
    pub max_execution_time: u32,
    pub max_input_time: u32,
    pub display_errors: bool,
    pub enabled_extensions: Vec<String>,
}

impl Default for PhpVisualSettings {
    fn default() -> Self {
        Self {
            memory_limit: "512M".to_string(),
            upload_max_filesize: "128M".to_string(),
            post_max_size: "128M".to_string(),
            max_execution_time: 300,
            max_input_time: 300,
            display_errors: true,
            enabled_extensions: vec![
                "curl".into(),
                "fileinfo".into(),
                "gd".into(),
                "intl".into(),
                "mbstring".into(),
                "exif".into(),
                "mysqli".into(),
                "openssl".into(),
                "pdo_mysql".into(),
                "pdo_sqlite".into(),
                "sqlite3".into(),
                "zip".into(),
            ],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MySqlVisualSettings {
    pub innodb_buffer_pool_size: String,
    pub max_allowed_packet: String,
    pub max_connections: u32,
}

impl Default for MySqlVisualSettings {
    fn default() -> Self {
        Self {
            innodb_buffer_pool_size: "256M".to_string(),
            max_allowed_packet: "64M".to_string(),
            max_connections: 150,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NginxVisualSettings {
    pub client_max_body_size: String,
    pub keepalive_timeout: u32,
}

impl Default for NginxVisualSettings {
    fn default() -> Self {
        Self {
            client_max_body_size: "128M".to_string(),
            keepalive_timeout: 65,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTarget {
    Php,
    Nginx,
    MySql,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PhpExtensionInfo {
    pub name: String,
    pub enabled: bool,
}

pub struct ConfigEditorManager {
    app_root: PathBuf,
    custom_dir: PathBuf,
}

impl ConfigEditorManager {
    pub fn new(app_root: impl Into<PathBuf>) -> Self {
        let root = app_root.into();
        let custom_dir = root.join("conf/custom");
        let _ = fs::create_dir_all(&custom_dir);
        Self {
            app_root: root,
            custom_dir,
        }
    }

    pub fn custom_file_path(&self, target: ConfigTarget) -> PathBuf {
        match target {
            ConfigTarget::Php => self.custom_dir.join("php_custom.ini"),
            ConfigTarget::Nginx => self.custom_dir.join("nginx_http_custom.conf"),
            ConfigTarget::MySql => self.custom_dir.join("my_custom.cnf"),
        }
    }

    /// Read custom raw config for the Code Editor UI.
    pub fn read_custom_raw(&self, target: ConfigTarget) -> String {
        let path = self.custom_file_path(target);
        fs::read_to_string(path).unwrap_or_else(|_| match target {
            ConfigTarget::Php => {
                "; Tambahkan directive php.ini kustom di bawah ini:\n; xdebug.mode = debug\n"
                    .to_string()
            }
            ConfigTarget::Nginx => {
                "# Tambahkan directive HTTP Nginx kustom di bawah ini:\n# server_tokens off;\n"
                    .to_string()
            }
            ConfigTarget::MySql => {
                "# Tambahkan directive [mysqld] kustom di bawah ini:\n".to_string()
            }
        })
    }

    /// Persist custom raw config from the Code Editor UI into `conf/custom/`.
    pub fn save_custom_raw(&self, target: ConfigTarget, content: &str) -> Result<()> {
        fs::create_dir_all(&self.custom_dir)?;
        let path = self.custom_file_path(target);
        fs::write(&path, content)
            .with_context(|| format!("Gagal menyimpan konfigurasi kustom ke {:?}", path))?;
        Ok(())
    }

    /// Scan PHP `ext/` for available `.dll` / `.so` modules.
    pub fn scan_available_php_extensions(
        &self,
        php_version: &str,
        enabled_list: &[String],
    ) -> Result<Vec<PhpExtensionInfo>> {
        let ext_dir = if cfg!(target_os = "windows") {
            self.app_root.join("bin/php").join(php_version).join("ext")
        } else {
            // Prefer common shared-extension layouts; static-php may have none.
            let candidates = [
                self.app_root
                    .join("bin/php")
                    .join(php_version)
                    .join("lib/php/extensions"),
                self.app_root.join("bin/php").join(php_version).join("ext"),
            ];
            candidates
                .into_iter()
                .find(|p| p.exists())
                .unwrap_or_else(|| {
                    self.app_root
                        .join("bin/php")
                        .join(php_version)
                        .join("lib/php/extensions")
                })
        };

        let mut results = Vec::new();
        if !ext_dir.exists() {
            return Ok(results);
        }

        // Some PHP builds nest versioned dirs under extensions/.
        let scan_dirs: Vec<PathBuf> = {
            let mut dirs = vec![ext_dir.clone()];
            if let Ok(entries) = fs::read_dir(&ext_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        dirs.push(entry.path());
                    }
                }
            }
            dirs
        };

        let mut seen = std::collections::HashSet::new();
        for dir in scan_dirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();

                let clean_name = if file_name.starts_with("php_") && file_name.ends_with(".dll") {
                    file_name
                        .trim_start_matches("php_")
                        .trim_end_matches(".dll")
                        .to_string()
                } else if file_name.ends_with(".so") {
                    file_name.trim_end_matches(".so").to_string()
                } else {
                    continue;
                };

                if clean_name == "opcache" || !seen.insert(clean_name.clone()) {
                    continue;
                }

                let is_enabled = enabled_list.iter().any(|e| e == &clean_name);
                results.push(PhpExtensionInfo {
                    name: clean_name,
                    enabled: is_enabled,
                });
            }
        }

        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    /// Validate nginx config with `nginx -t` before applying raw edits.
    pub async fn validate_nginx_config(&self) -> Result<()> {
        let bin_name = if cfg!(target_os = "windows") {
            "nginx.exe"
        } else {
            "nginx"
        };
        let nginx_bin = self.app_root.join("bin/nginx").join(bin_name);
        if !nginx_bin.exists() {
            return Ok(());
        }

        let nginx_prefix = self
            .app_root
            .join("bin/nginx")
            .to_string_lossy()
            .replace('\\', "/");
        let nginx_conf = self
            .app_root
            .join("conf/nginx.conf")
            .to_string_lossy()
            .replace('\\', "/");

        let mut cmd = Command::new(nginx_bin);
        cmd.args(["-t", "-p", &nginx_prefix, "-c", &nginx_conf]);

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd.output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if !stderr.trim().is_empty() {
                stderr.to_string()
            } else {
                stdout.to_string()
            };
            bail!("Syntax Nginx Error:\n{}", detail);
        }

        Ok(())
    }

    /// Open the rendered config file in the OS text editor.
    pub fn open_in_external_editor(&self, target: ConfigTarget) -> Result<()> {
        let rel_path = match target {
            ConfigTarget::Php => "conf/php.ini",
            ConfigTarget::Nginx => "conf/nginx.conf",
            ConfigTarget::MySql => "conf/my.cnf",
        };

        let target_file = self.app_root.join(rel_path);
        if !target_file.exists() {
            bail!("File konfigurasi belum di-generate: {:?}", target_file);
        }

        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", &target_file.to_string_lossy()])
                .spawn()?;
        }

        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-t")
                .arg(&target_file)
                .spawn()?;
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            std::process::Command::new("xdg-open")
                .arg(&target_file)
                .spawn()?;
        }

        Ok(())
    }

    pub fn app_root(&self) -> &Path {
        &self.app_root
    }
}

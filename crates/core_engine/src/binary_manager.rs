use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const BUNDLED_MANIFEST: &str = include_str!("../resources/binaries_manifest.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub component: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percentage: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub version: String,
    pub url: Option<String>,
    pub target_dir: String,
    pub strip_root: bool,
    #[serde(default = "default_archive")]
    pub archive: String,
    #[serde(default)]
    pub note: Option<String>,
}

fn default_archive() -> String {
    "zip".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformManifest {
    pub nginx: ComponentSpec,
    pub mariadb: ComponentSpec,
    pub php: std::collections::HashMap<String, ComponentSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinariesManifest {
    pub windows_x64: PlatformManifest,
    pub macos_aarch64: PlatformManifest,
    pub macos_x86_64: PlatformManifest,
}

impl BinariesManifest {
    pub fn bundled() -> Result<Self> {
        serde_json::from_str(BUNDLED_MANIFEST).context("failed to parse bundled binaries_manifest.json")
    }

    pub fn for_current_platform(&self) -> &PlatformManifest {
        if cfg!(target_os = "windows") {
            &self.windows_x64
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            &self.macos_aarch64
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            &self.macos_x86_64
        } else {
            &self.macos_x86_64
        }
    }
}

pub struct BinaryManager {
    app_root: PathBuf,
    http_client: reqwest::Client,
    manifest: BinariesManifest,
}

impl BinaryManager {
    pub fn new(app_root: impl Into<PathBuf>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("praews/0.1 (Portable MAMP Alternative)")
            .build()
            .unwrap_or_default();

        Ok(Self {
            app_root: app_root.into(),
            http_client: client,
            manifest: BinariesManifest::bundled()?,
        })
    }

    pub fn manifest(&self) -> &BinariesManifest {
        &self.manifest
    }

    pub async fn download_file<F>(
        &self,
        url: &str,
        dest_file: &Path,
        component_name: &str,
        mut on_progress: F,
    ) -> Result<()>
    where
        F: FnMut(DownloadProgress),
    {
        if let Some(parent) = dest_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to contact URL: {url}"))?;

        if !response.status().is_success() {
            bail!("download failed with HTTP {}: {url}", response.status());
        }

        let total_bytes = response.content_length();
        let mut downloaded_bytes: u64 = 0;
        let mut file = tokio::fs::File::create(dest_file)
            .await
            .with_context(|| format!("failed to create {:?}", dest_file))?;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed reading download stream")?;
            file.write_all(&chunk).await?;
            downloaded_bytes += chunk.len() as u64;
            let percentage = match total_bytes {
                Some(total) if total > 0 => (downloaded_bytes as f32 / total as f32) * 100.0,
                _ => 0.0,
            };
            on_progress(DownloadProgress {
                component: component_name.to_string(),
                downloaded_bytes,
                total_bytes,
                percentage,
            });
        }

        file.flush().await?;
        Ok(())
    }

    pub fn extract_zip(zip_path: &Path, target_dir: &Path, strip_top_level_dir: bool) -> Result<()> {
        let file = File::open(zip_path)
            .with_context(|| format!("failed to open zip {:?}", zip_path))?;
        let mut archive = zip::ZipArchive::new(file).context("invalid zip archive")?;
        fs::create_dir_all(target_dir)?;

        for i in 0..archive.len() {
            let mut zip_file = archive.by_index(i)?;
            let enclosed_path = match zip_file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };

            let relative_path = if strip_top_level_dir {
                let mut components = enclosed_path.components();
                components.next();
                components.as_path().to_path_buf()
            } else {
                enclosed_path
            };

            if relative_path.as_os_str().is_empty() {
                continue;
            }

            let outpath = target_dir.join(&relative_path);
            if zip_file.name().ends_with('/') {
                fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    fs::create_dir_all(p)?;
                }
                let mut outfile = File::create(&outpath)?;
                io::copy(&mut zip_file, &mut outfile)?;
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = zip_file.unix_mode() {
                    fs::set_permissions(&outpath, fs::Permissions::from_mode(mode))?;
                }
            }
        }

        Ok(())
    }

    pub fn extract_tar_gz(
        archive_path: &Path,
        target_dir: &Path,
        strip_top_level_dir: bool,
    ) -> Result<()> {
        let file = File::open(archive_path)
            .with_context(|| format!("failed to open archive {:?}", archive_path))?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        fs::create_dir_all(target_dir)?;

        for entry in archive.entries()? {
            let mut entry = entry?;
            let enclosed = entry.path()?.into_owned();
            let relative = if strip_top_level_dir {
                let mut components = enclosed.components();
                components.next();
                components.as_path().to_path_buf()
            } else {
                enclosed
            };

            if relative.as_os_str().is_empty() {
                continue;
            }

            let outpath = target_dir.join(&relative);
            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut outfile = File::create(&outpath)?;
                io::copy(&mut entry, &mut outfile)?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(mode) = entry.header().mode() {
                        fs::set_permissions(&outpath, fs::Permissions::from_mode(mode))?;
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn ensure_cacert_bundle(&self) -> Result<()> {
        let cacert_path = self.app_root.join("conf/ssl/cacert.pem");
        if cacert_path.exists() {
            return Ok(());
        }
        if let Some(parent) = cacert_path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.download_file(
            "https://curl.se/ca/cacert.pem",
            &cacert_path,
            "SSL CA Bundle",
            |_| {},
        )
        .await?;
        Ok(())
    }

    pub async fn install_component<F>(
        &self,
        component_name: &str,
        download_url: &str,
        target_rel_dir: &str,
        strip_root_folder: bool,
        archive_kind: &str,
        on_progress: F,
    ) -> Result<()>
    where
        F: FnMut(DownloadProgress),
    {
        let ext = if archive_kind == "tar_gz" {
            "tar.gz"
        } else {
            "zip"
        };
        let temp_archive = self
            .app_root
            .join("temp")
            .join(format!("{component_name}.{ext}"));
        let target_dir = self.app_root.join(target_rel_dir);

        self.download_file(download_url, &temp_archive, component_name, on_progress)
            .await?;

        let zip_clone = temp_archive.clone();
        let target_clone = target_dir.clone();
        let kind = archive_kind.to_string();
        tokio::task::spawn_blocking(move || {
            if kind == "tar_gz" {
                Self::extract_tar_gz(&zip_clone, &target_clone, strip_root_folder)
            } else {
                Self::extract_zip(&zip_clone, &target_clone, strip_root_folder)
            }
        })
        .await??;

        let _ = fs::remove_file(&temp_archive);
        Ok(())
    }

    pub async fn install_from_spec<F>(&self, name: &str, spec: &ComponentSpec, on_progress: F) -> Result<()>
    where
        F: FnMut(DownloadProgress),
    {
        let Some(url) = spec.url.as_deref() else {
            let note = spec
                .note
                .clone()
                .unwrap_or_else(|| "no download URL configured for this platform".into());
            bail!("cannot install {name}: {note}");
        };

        self.install_component(
            name,
            url,
            &spec.target_dir,
            spec.strip_root,
            &spec.archive,
            on_progress,
        )
        .await
    }

    /// Initialize MySQL/MariaDB system tables when `data/mysql/mysql` is missing.
    pub async fn initialize_mysql_datadir(&self) -> Result<()> {
        let data_dir = self.app_root.join("data/mysql");
        let system_table_check = data_dir.join("mysql");
        if system_table_check.exists() {
            return Ok(());
        }

        fs::create_dir_all(&data_dir)?;
        let mysql_bin_dir = self.app_root.join("bin/mysql/bin");
        let my_cnf = self
            .app_root
            .join("conf/my.cnf")
            .to_string_lossy()
            .replace('\\', "/");
        let clean_datadir = data_dir.to_string_lossy().replace('\\', "/");

        let mariadb_installer = if cfg!(target_os = "windows") {
            mysql_bin_dir.join("mysql_install_db.exe")
        } else {
            let candidates = [
                mysql_bin_dir.join("mariadb-install-db"),
                mysql_bin_dir.join("mysql_install_db"),
            ];
            candidates
                .into_iter()
                .find(|p| p.exists())
                .unwrap_or_else(|| mysql_bin_dir.join("mariadb-install-db"))
        };

        let mut cmd = if mariadb_installer.exists() {
            let mut c = Command::new(&mariadb_installer);
            c.arg(format!("--datadir={clean_datadir}"));
            #[cfg(not(target_os = "windows"))]
            {
                let basedir = self
                    .app_root
                    .join("bin/mysql")
                    .to_string_lossy()
                    .replace('\\', "/");
                c.arg(format!("--basedir={basedir}"));
                c.arg("--auth-root-authentication-method=normal");
            }
            c
        } else {
            let mysqld_bin = if cfg!(target_os = "windows") {
                mysql_bin_dir.join("mysqld.exe")
            } else {
                mysql_bin_dir.join("mysqld")
            };
            if !mysqld_bin.exists() {
                bail!(
                    "MySQL/MariaDB binaries not found under {:?}. Install them first.",
                    mysql_bin_dir
                );
            }
            let mut c = Command::new(mysqld_bin);
            c.arg(format!("--defaults-file={my_cnf}"))
                .arg("--initialize-insecure")
                .arg("--console");
            c
        };

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd
            .output()
            .await
            .context("failed to run database initializer")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!("MySQL initialization failed!\nSTDOUT: {stdout}\nSTDERR: {stderr}");
        }

        Ok(())
    }

    /// Stop PHP, re-render configs for `version`, start PHP again.
    pub async fn switch_php_version(
        &self,
        orchestrator: &crate::orchestrator::ProcessOrchestrator,
        version: &str,
        http_port: u16,
        php_port: u16,
        mysql_port: u16,
    ) -> Result<()> {
        let _ = orchestrator
            .stop_service(crate::orchestrator::ServiceType::Php)
            .await;

        let config = crate::config_engine::SandboxConfig::new(&self.app_root, version)
            .with_ports(http_port, php_port, mysql_port);
        crate::config_engine::render_all_configs(&self.app_root, &config)?;

        // Reload nginx so FastCGI upstream / php.ini paths stay consistent.
        if self.app_root.join("bin/nginx").exists() {
            let _ = orchestrator.stop_service(crate::orchestrator::ServiceType::Nginx).await;
            let _ = orchestrator.start_nginx(http_port).await;
        }

        orchestrator.start_php(version, php_port).await?;
        Ok(())
    }

    pub fn is_component_installed(&self, target_rel_dir: &str) -> bool {
        let dir = self.app_root.join(target_rel_dir);
        dir.exists()
            && fs::read_dir(&dir)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false)
    }
}

/// Helper used by CLI progress printing.
pub fn print_progress(p: DownloadProgress) {
    if let Some(total) = p.total_bytes {
        eprint!(
            "\r[{}] {:.1}% ({}/{} bytes)",
            p.component, p.percentage, p.downloaded_bytes, total
        );
    } else {
        eprint!(
            "\r[{}] {} bytes downloaded",
            p.component, p.downloaded_bytes
        );
    }
    let _ = io::Write::flush(&mut io::stderr());
}

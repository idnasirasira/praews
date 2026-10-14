use anyhow::{Context, Result};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Process-scoped CLI environment (PATH / PHPRC / Composer) without touching OS globals.
pub struct CliEnvManager {
    app_root: PathBuf,
}

impl CliEnvManager {
    pub fn new(app_root: impl Into<PathBuf>) -> Self {
        Self {
            app_root: app_root.into(),
        }
    }

    pub fn shims_dir(&self) -> PathBuf {
        self.app_root.join("bin/shims")
    }

    /// Create Windows + Unix composer wrapper scripts under `bin/shims/`.
    pub fn ensure_composer_shims(&self) -> Result<PathBuf> {
        let shims_dir = self.shims_dir();
        fs::create_dir_all(&shims_dir)?;

        let bat_path = shims_dir.join("composer.bat");
        fs::write(&bat_path, "@echo off\r\nphp \"%~dp0composer.phar\" %*\r\n")?;

        let sh_path = shims_dir.join("composer");
        let sh_content = r#"#!/bin/sh
basedir=$(dirname "$(echo "$0" | sed -e 's,\\,/,g')")
php "$basedir/composer.phar" "$@"
"#;
        fs::write(&sh_path, sh_content)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sh_path, fs::Permissions::from_mode(0o755))?;
        }

        Ok(shims_dir)
    }

    /// Download `composer.phar` into shims if missing.
    pub async fn ensure_composer_phar(&self, http: &reqwest::Client) -> Result<PathBuf> {
        let shims = self.ensure_composer_shims()?;
        let phar = shims.join("composer.phar");
        if phar.exists() {
            return Ok(phar);
        }

        let response = http
            .get("https://getcomposer.org/download/latest-stable/composer.phar")
            .send()
            .await
            .context("failed to download composer.phar")?;
        if !response.status().is_success() {
            anyhow::bail!("composer download HTTP {}", response.status());
        }
        let bytes = response.bytes().await?;
        fs::write(&phar, bytes)?;
        Ok(phar)
    }

    /// Build env vars that only apply to child processes (never written to the OS).
    pub fn build_scoped_env(
        &self,
        active_php_version: &str,
        mysql_port: u16,
    ) -> Result<HashMap<String, String>> {
        let shims_dir = self.ensure_composer_shims()?;
        let php_dir = self.app_root.join("bin/php").join(active_php_version);
        let php_bin_dir = if cfg!(target_os = "windows") {
            php_dir.clone()
        } else {
            // Prefer bin/, fall back to root of version dir.
            let bin = php_dir.join("bin");
            if bin.exists() {
                bin
            } else {
                php_dir.join("sbin")
            }
        };

        let mysql_bin_dir = self.app_root.join("bin/mysql/bin");
        let nginx_bin_dir = self.app_root.join("bin/nginx");
        let node_bin_dir = self.app_root.join("bin/nodejs");

        let current_os_path = env::var_os("PATH").unwrap_or_default();
        let mut custom_paths = vec![
            shims_dir,
            php_bin_dir,
            mysql_bin_dir,
            nginx_bin_dir,
            node_bin_dir,
        ];
        custom_paths.extend(env::split_paths(&current_os_path));
        let new_path_os =
            env::join_paths(custom_paths).context("failed to join scoped PATH")?;

        let mut env_vars = HashMap::new();
        env_vars.insert("PATH".to_string(), new_path_os.to_string_lossy().into_owned());

        // Force PHP CLI to use sandbox php.ini (same as Nginx FastCGI).
        env_vars.insert(
            "PHPRC".to_string(),
            self.app_root.join("conf").to_string_lossy().into_owned(),
        );

        // Point `mysql` / `mariadb` clients at the sandbox MySQL port.
        env_vars.insert("MYSQL_TCP_PORT".to_string(), mysql_port.to_string());
        env_vars.insert("MYSQL_HOST".to_string(), "127.0.0.1".to_string());
        env_vars.insert(
            "MYSQL_UNIX_PORT".to_string(),
            self.app_root
                .join("temp/mysql/mysql.sock")
                .to_string_lossy()
                .into_owned(),
        );

        let cacert_path = self.app_root.join("conf/ssl/cacert.pem");
        if cacert_path.exists() {
            let cert_str = cacert_path.to_string_lossy().into_owned();
            env_vars.insert("SSL_CERT_FILE".to_string(), cert_str.clone());
            env_vars.insert("CURL_CA_BUNDLE".to_string(), cert_str);
        }

        let composer_home = self.app_root.join("temp/composer");
        let _ = fs::create_dir_all(&composer_home);
        env_vars.insert(
            "COMPOSER_HOME".to_string(),
            composer_home.to_string_lossy().into_owned(),
        );

        Ok(env_vars)
    }

    /// Open an OS terminal in `project_dir` with the scoped environment injected.
    pub fn open_project_terminal(
        &self,
        project_dir: &Path,
        active_php_version: &str,
        mysql_port: u16,
    ) -> Result<()> {
        let scoped_env = self.build_scoped_env(active_php_version, mysql_port)?;

        #[cfg(target_os = "windows")]
        {
            let has_wt = Command::new("where")
                .arg("wt.exe")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            let banner = format!(
                "Write-Host 'praews CLI (PHP {active_php_version}, MySQL :{mysql_port}) ready' -ForegroundColor Green"
            );

            let mut cmd = if has_wt {
                let mut c = Command::new("wt.exe");
                c.arg("-d")
                    .arg(project_dir)
                    .arg("powershell.exe")
                    .arg("-NoExit")
                    .arg("-Command")
                    .arg(&banner);
                c
            } else {
                let mut c = Command::new("cmd.exe");
                c.args(["/C", "start", "powershell.exe", "-NoExit", "-Command"])
                    .arg(&banner);
                c
            };
            cmd.current_dir(project_dir);
            cmd.envs(&scoped_env);
            cmd.spawn().context("failed to open Windows terminal")?;
        }

        #[cfg(target_os = "macos")]
        {
            let temp_dir = self.app_root.join("temp");
            fs::create_dir_all(&temp_dir)?;
            let rc_path = temp_dir.join("sandbox_shell_init.sh");

            let path_val = scoped_env.get("PATH").context("PATH missing")?;
            let phprc_val = scoped_env.get("PHPRC").context("PHPRC missing")?;
            let composer_home = scoped_env
                .get("COMPOSER_HOME")
                .context("COMPOSER_HOME missing")?;
            let mysql_tcp = scoped_env
                .get("MYSQL_TCP_PORT")
                .context("MYSQL_TCP_PORT missing")?;
            let mysql_host = scoped_env
                .get("MYSQL_HOST")
                .context("MYSQL_HOST missing")?;
            let mysql_sock = scoped_env
                .get("MYSQL_UNIX_PORT")
                .cloned()
                .unwrap_or_default();
            let ssl_cert = scoped_env
                .get("SSL_CERT_FILE")
                .cloned()
                .unwrap_or_default();
            let curl_ca = scoped_env
                .get("CURL_CA_BUNDLE")
                .cloned()
                .unwrap_or_default();

            let init_script = format!(
                "#!/bin/zsh\n\
export PATH=\"{path_val}\"\n\
export PHPRC=\"{phprc_val}\"\n\
export COMPOSER_HOME=\"{composer_home}\"\n\
export MYSQL_TCP_PORT=\"{mysql_tcp}\"\n\
export MYSQL_HOST=\"{mysql_host}\"\n\
export MYSQL_UNIX_PORT=\"{mysql_sock}\"\n\
export SSL_CERT_FILE=\"{ssl_cert}\"\n\
export CURL_CA_BUNDLE=\"{curl_ca}\"\n\
cd \"{}\"\n\
clear\n\
echo \"\\033[1;32mpraews CLI (PHP {active_php_version}, MySQL :{mysql_port}) ready\\033[0m\"\n\
exec /bin/zsh -i\n",
                project_dir.to_string_lossy()
            );
            fs::write(&rc_path, init_script)?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&rc_path, fs::Permissions::from_mode(0o755))?;

            Command::new("open")
                .arg("-a")
                .arg("Terminal")
                .arg(&rc_path)
                .spawn()
                .context("failed to open Terminal.app")?;
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let mut bash = Command::new("bash");
            bash.arg("-lc")
                .arg(format!("cd '{}' && exec bash -i", project_dir.display()))
                .envs(&scoped_env)
                .spawn()
                .context("failed to open bash terminal")?;
        }

        Ok(())
    }

    /// Print a shell snippet the user can `eval` (optional; still process-scoped if sourced).
    pub fn export_shell_snippet(
        &self,
        active_php_version: &str,
        mysql_port: u16,
    ) -> Result<String> {
        let env = self.build_scoped_env(active_php_version, mysql_port)?;
        let mut lines = Vec::new();
        for key in [
            "PATH",
            "PHPRC",
            "COMPOSER_HOME",
            "MYSQL_TCP_PORT",
            "MYSQL_HOST",
            "MYSQL_UNIX_PORT",
            "SSL_CERT_FILE",
            "CURL_CA_BUNDLE",
        ] {
            if let Some(val) = env.get(key) {
                lines.push(format!("export {key}=\"{val}\""));
            }
        }
        Ok(lines.join("\n"))
    }
}

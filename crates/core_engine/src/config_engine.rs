use crate::sandbox::{init_sandbox, normalize_path};
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;
use tera::{Context as TeraContext, Tera};

#[derive(Debug, Clone, Serialize)]
pub struct SandboxConfig {
    pub app_root: String,
    pub http_port: u16,
    pub https_port: u16,
    pub php_fastcgi_port: u16,
    pub mysql_port: u16,
    pub php_version: String,
    pub timezone: String,
    pub is_windows: bool,
}

impl SandboxConfig {
    pub fn new(app_root_path: &Path, php_version: &str) -> Self {
        // Prefer absolute paths so configs stay valid after cwd changes / GUI launch.
        let absolute = std::fs::canonicalize(app_root_path).unwrap_or_else(|_| {
            if app_root_path.is_absolute() {
                app_root_path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_default()
                    .join(app_root_path)
            }
        });

        Self {
            app_root: normalize_path(&absolute),
            http_port: 8888,
            https_port: 8443,
            php_fastcgi_port: 9000,
            mysql_port: 3306,
            php_version: php_version.to_string(),
            timezone: "Asia/Jakarta".to_string(),
            is_windows: cfg!(target_os = "windows"),
        }
    }

    pub fn with_ports(mut self, http: u16, php_fastcgi: u16, mysql: u16) -> Self {
        self.http_port = http;
        self.php_fastcgi_port = php_fastcgi;
        self.mysql_port = mysql;
        self
    }

    pub fn with_https_port(mut self, https: u16) -> Self {
        self.https_port = https;
        self
    }
}

/// Render core service configs from `templates/*.tera` into `conf/`.
pub fn render_all_configs(app_root: &Path, config: &SandboxConfig) -> Result<()> {
    init_sandbox(app_root)?;

    let template_glob = format!("{}/templates/*.tera", config.app_root);
    let tera = Tera::new(&template_glob).context("failed to load Tera templates")?;
    let context = TeraContext::from_serialize(config)
        .context("failed to serialize SandboxConfig into Tera context")?;

    let targets = [
        ("nginx.conf.tera", "conf/nginx.conf"),
        ("php.ini.tera", "conf/php.ini"),
        ("php-fpm.conf.tera", "conf/php-fpm.conf"),
        ("my.cnf.tera", "conf/my.cnf"),
    ];

    for (template_name, output_rel_path) in targets {
        let rendered = tera
            .render(template_name, &context)
            .with_context(|| format!("failed to render template {template_name}"))?;
        let output_path = app_root.join(output_rel_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {:?}", parent))?;
        }
        fs::write(&output_path, rendered)
            .with_context(|| format!("failed to write config {:?}", output_path))?;
        tracing::info!(path = %output_path.display(), "wrote config");
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct VhostConfig {
    pub app_root: String,
    pub http_port: u16,
    pub https_port: u16,
    pub php_fastcgi_port: u16,
    pub domain: String,
    pub project_public_path: String,
}

/// Render a single virtual host config into `conf/vhosts/{domain}.conf`.
pub fn render_vhost(app_root: &Path, vhost: &VhostConfig) -> Result<std::path::PathBuf> {
    init_sandbox(app_root)?;

    let absolute = std::fs::canonicalize(app_root).unwrap_or_else(|_| app_root.to_path_buf());
    let template_glob = format!("{}/templates/*.tera", normalize_path(&absolute));
    let tera = Tera::new(&template_glob).context("failed to load Tera templates")?;
    let context = TeraContext::from_serialize(vhost)
        .context("failed to serialize VhostConfig into Tera context")?;

    let rendered = tera
        .render("vhost.conf.tera", &context)
        .context("failed to render vhost.conf.tera")?;

    let output = app_root
        .join("conf/vhosts")
        .join(format!("{}.conf", vhost.domain));
    fs::write(&output, rendered)
        .with_context(|| format!("failed to write vhost {:?}", output))?;
    tracing::info!(path = %output.display(), "wrote vhost config");
    Ok(output)
}

/// Re-render every virtual host when HTTP/HTTPS/FastCGI ports change.
pub fn render_all_vhosts(
    app_root: &Path,
    config: &SandboxConfig,
    vhosts: &[(String, String)],
) -> Result<()> {
    for (domain, public_path) in vhosts {
        let vhost = VhostConfig {
            app_root: config.app_root.clone(),
            http_port: config.http_port,
            https_port: config.https_port,
            php_fastcgi_port: config.php_fastcgi_port,
            domain: domain.clone(),
            project_public_path: public_path.clone(),
        };
        render_vhost(app_root, &vhost)?;
    }
    Ok(())
}

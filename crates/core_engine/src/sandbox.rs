use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Directories required for a zero-pollution portable sandbox.
pub const REQUIRED_DIRS: &[&str] = &[
    "bin/nginx",
    "bin/php",
    "bin/mysql",
    "bin/shims",
    "temp/composer",
    "conf/vhosts",
    "conf/ssl",
    "conf/custom",
    "templates",
    "logs",
    "htdocs",
    "data/mysql",
    "temp/nginx/client_body",
    "temp/nginx/proxy",
    "temp/nginx/fastcgi",
    "temp/nginx/uwsgi",
    "temp/nginx/scgi",
    "temp/php/uploads",
    "temp/php/sessions",
    "temp/mysql",
];

const BUNDLED_TEMPLATES: &[(&str, &str)] = &[
    ("nginx.conf.tera", include_str!("../templates/nginx.conf.tera")),
    ("php.ini.tera", include_str!("../templates/php.ini.tera")),
    ("php-fpm.conf.tera", include_str!("../templates/php-fpm.conf.tera")),
    ("my.cnf.tera", include_str!("../templates/my.cnf.tera")),
    ("vhost.conf.tera", include_str!("../templates/vhost.conf.tera")),
];

/// Resolve the portable app root.
///
/// Priority:
/// 1. `PRAEWS_ROOT` environment variable
/// 2. Directory containing the current executable
/// 3. Current working directory
pub fn resolve_app_root() -> Result<PathBuf> {
    if let Ok(from_env) = env::var("PRAEWS_ROOT") {
        return Ok(PathBuf::from(from_env));
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            return Ok(parent.to_path_buf());
        }
    }

    env::current_dir().context("failed to resolve current working directory")
}

/// Create the full sandbox layout under `app_root`.
pub fn init_sandbox(app_root: &Path) -> Result<()> {
    for dir in REQUIRED_DIRS {
        fs::create_dir_all(app_root.join(dir))
            .with_context(|| format!("failed to create sandbox directory: {dir}"))?;
    }

    seed_default_templates(app_root)?;
    // Always refresh php-fpm template if we added it after an older sandbox seed.
    let fpm_tpl = app_root.join("templates/php-fpm.conf.tera");
    if !fpm_tpl.exists() {
        fs::write(
            &fpm_tpl,
            include_str!("../templates/php-fpm.conf.tera"),
        )?;
    }

    let index = app_root.join("htdocs/index.php");
    if !index.exists() {
        fs::write(
            &index,
            "<?php\nphpinfo();\n",
        )
        .with_context(|| format!("failed to write default htdocs page at {:?}", index))?;
    }

    tracing::info!(root = %app_root.display(), "sandbox initialized");
    Ok(())
}

/// Copy bundled Tera templates into `app_root/templates/` when missing.
pub fn seed_default_templates(app_root: &Path) -> Result<()> {
    let templates_dir = app_root.join("templates");
    fs::create_dir_all(&templates_dir)
        .with_context(|| format!("failed to create templates dir at {:?}", templates_dir))?;

    for (name, contents) in BUNDLED_TEMPLATES {
        let path = templates_dir.join(name);
        if !path.exists() {
            fs::write(&path, contents)
                .with_context(|| format!("failed to seed template {:?}", path))?;
        }
    }

    Ok(())
}

/// Normalize Windows backslashes to forward slashes for Nginx/PHP/MySQL configs.
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

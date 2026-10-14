use crate::config_engine::{render_vhost, VhostConfig};
use crate::dns_server::get_local_lan_ip;
use crate::sandbox::{init_sandbox, normalize_path};
use crate::ssl_manager::SslManager;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteRecord {
    pub domain: String,
    pub project_path: PathBuf,
    pub public_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SiteState {
    pub sites: Vec<SiteRecord>,
}

/// Detect `public/` for modern PHP frameworks, otherwise use project root.
pub fn detect_public_path(project_path: &Path) -> PathBuf {
    let public = project_path.join("public");
    if public.is_dir() {
        public
    } else {
        project_path.to_path_buf()
    }
}

pub fn state_path(app_root: &Path) -> PathBuf {
    app_root.join("state.json")
}

pub fn load_state(app_root: &Path) -> Result<SiteState> {
    let path = state_path(app_root);
    if !path.exists() {
        return Ok(SiteState::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {:?}", path))?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

pub fn save_state(app_root: &Path, state: &SiteState) -> Result<()> {
    let path = state_path(app_root);
    let raw = serde_json::to_string_pretty(state)?;
    fs::write(&path, raw).with_context(|| format!("failed to write {:?}", path))?;
    Ok(())
}

/// Register a virtual host, issue SSL cert, and render nginx vhost config.
pub fn add_site(
    app_root: &Path,
    domain: &str,
    project_path: &Path,
    http_port: u16,
    https_port: u16,
    php_fastcgi_port: u16,
) -> Result<SiteRecord> {
    init_sandbox(app_root)?;

    let project_path = fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let public_path = detect_public_path(&project_path);

    let ssl = SslManager::new(app_root);
    let lan_ip = get_local_lan_ip();
    ssl.generate_domain_cert(domain, lan_ip)?;

    let absolute = fs::canonicalize(app_root).unwrap_or_else(|_| app_root.to_path_buf());
    let vhost = VhostConfig {
        app_root: normalize_path(&absolute),
        http_port,
        https_port,
        php_fastcgi_port,
        domain: domain.to_string(),
        project_public_path: normalize_path(&public_path),
    };
    render_vhost(app_root, &vhost)?;

    let record = SiteRecord {
        domain: domain.to_string(),
        project_path,
        public_path,
    };

    let mut state = load_state(app_root)?;
    state.sites.retain(|s| s.domain != domain);
    state.sites.push(record.clone());
    save_state(app_root, &state)?;

    Ok(record)
}

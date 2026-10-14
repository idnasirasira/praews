use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::sandbox::REQUIRED_DIRS;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PortableCheck {
    pub ok: bool,
    pub app_root: String,
    pub missing_dirs: Vec<String>,
    pub notes: Vec<String>,
}

/// Verify the sandbox layout is self-contained (delete folder = clean machine).
pub fn verify_portable_layout(app_root: &Path) -> Result<PortableCheck> {
    let absolute = fs::canonicalize(app_root).unwrap_or_else(|_| app_root.to_path_buf());
    let mut missing = Vec::new();
    for dir in REQUIRED_DIRS {
        if !absolute.join(dir).exists() {
            missing.push((*dir).to_string());
        }
    }

    let mut notes = vec![
        "All runtime data must stay under app_root (bin/conf/data/logs/temp/htdocs).".into(),
        "Relocate test: stop services, move the folder, start again — paths are re-rendered absolute on start.".into(),
        "Optional OS Root CA trust is reversible via SslManager::untrust_root_ca_from_os.".into(),
    ];

    if cfg!(target_os = "macos") {
        notes.push(
            "For *.test without root DNS: create /etc/resolver/test with nameserver 127.0.0.1 and port 53535 (one-time, scoped)."
                .into(),
        );
    }

    let check = PortableCheck {
        ok: missing.is_empty(),
        app_root: absolute.display().to_string(),
        missing_dirs: missing,
        notes,
    };

    if !check.ok {
        bail!(
            "portable layout incomplete under {}: missing {:?}",
            check.app_root,
            check.missing_dirs
        );
    }

    Ok(check)
}

/// Paths that must never be used for praews state (system pollution).
pub fn forbidden_system_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(home).join("Library/Application Support/praews"));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(PathBuf::from(appdata).join("praews"));
    }
    paths
}

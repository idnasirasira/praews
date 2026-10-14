use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::sandbox::{init_sandbox, REQUIRED_DIRS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocateMode {
    /// Use `new_root` as-is (create layout if empty). Do not copy from the old root.
    Point,
    /// Copy the current sandbox tree into `new_root`, then switch.
    Copy,
    /// Copy then remove the old sandbox (rename when possible).
    Move,
}

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

fn is_forbidden_root(path: &Path) -> bool {
    let Ok(canon) = fs::canonicalize(path).or_else(|_| {
        path.parent()
            .map(|p| fs::canonicalize(p).map(|c| c.join(path.file_name().unwrap_or_default())))
            .unwrap_or_else(|| Ok(path.to_path_buf()))
    }) else {
        return false;
    };
    for banned in forbidden_system_paths() {
        if canon.starts_with(&banned) || banned.starts_with(&canon) {
            return true;
        }
    }
    false
}

/// Recursively copy `src` into `dst` (dst is created).
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)
        .with_context(|| format!("failed to create destination {}", dst.display()))?;
    for entry in fs::read_dir(src)
        .with_context(|| format!("failed to read source {}", src.display()))?
    {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_file() {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)
                .with_context(|| format!("failed to copy {} → {}", from.display(), to.display()))?;
        } else if ty.is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(&from)?;
                let _ = fs::remove_file(&to);
                std::os::unix::fs::symlink(&target, &to).with_context(|| {
                    format!("failed to symlink {} → {}", target.display(), to.display())
                })?;
            }
            #[cfg(not(unix))]
            {
                fs::copy(&from, &to)?;
            }
        }
    }
    Ok(())
}

fn remove_dir_contents(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&p)
                .with_context(|| format!("failed to remove {}", p.display()))?;
        } else {
            fs::remove_file(&p).with_context(|| format!("failed to remove {}", p.display()))?;
        }
    }
    Ok(())
}

/// Prepare `new_root` as the sandbox folder according to `mode`.
///
/// Returns the absolute path that should become the new app root.
pub fn relocate_sandbox(old_root: &Path, new_root: &Path, mode: RelocateMode) -> Result<PathBuf> {
    if new_root.as_os_str().is_empty() {
        bail!("new root path is empty");
    }

    fs::create_dir_all(new_root)
        .with_context(|| format!("cannot create {}", new_root.display()))?;

    let new_abs = fs::canonicalize(new_root)
        .with_context(|| format!("cannot resolve {}", new_root.display()))?;
    let old_abs = fs::canonicalize(old_root).unwrap_or_else(|_| old_root.to_path_buf());

    if new_abs == old_abs {
        bail!("new root is the same as the current sandbox");
    }
    if old_abs.starts_with(&new_abs) || new_abs.starts_with(&old_abs) {
        bail!("new root must not be nested inside the current sandbox (or vice versa)");
    }
    if is_forbidden_root(&new_abs) {
        bail!("refusing system AppData / Application Support paths — keep the sandbox portable");
    }

    match mode {
        RelocateMode::Point => {
            init_sandbox(&new_abs)?;
        }
        RelocateMode::Copy | RelocateMode::Move => {
            // If destination already has content, refuse to clobber blindly.
            let is_empty = fs::read_dir(&new_abs)?.next().is_none();
            if !is_empty {
                // Allow reuse only when it already looks like a praews sandbox.
                let looks_like_sandbox = new_abs.join("conf").exists()
                    || new_abs.join("bin").exists()
                    || new_abs.join("state.json").exists();
                if looks_like_sandbox {
                    bail!(
                        "destination already looks like a praews sandbox. Use mode \"point\" to switch to it without copying."
                    );
                }
                bail!("destination folder is not empty: {}", new_abs.display());
            }

            if mode == RelocateMode::Move {
                match fs::rename(&old_abs, &new_abs) {
                    Ok(()) => {
                        // Recreate empty old path so callers that still hold the path don't break mid-restart.
                        let _ = fs::create_dir_all(&old_abs);
                        init_sandbox(&new_abs)?;
                        return Ok(new_abs);
                    }
                    Err(_) => {
                        // Cross-device: fall through to copy + delete.
                    }
                }
            }

            copy_dir_recursive(&old_abs, &new_abs)?;
            init_sandbox(&new_abs)?;

            if mode == RelocateMode::Move {
                remove_dir_contents(&old_abs)?;
            }
        }
    }

    Ok(new_abs)
}

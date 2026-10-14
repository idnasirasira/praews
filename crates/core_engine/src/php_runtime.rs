//! Locate PHP homes (sandbox, custom path, Homebrew layout) and macOS dyld hints.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// True when `dir` contains a usable CLI + FastCGI/FPM binary.
pub fn php_home_is_valid(dir: &Path) -> bool {
    find_php_cli(dir).is_some() && find_php_fastcgi(dir).is_some()
}

pub fn find_php_cli(dir: &Path) -> Option<PathBuf> {
    [
        dir.join("php.exe"),
        dir.join("php"),
        dir.join("bin/php"),
        dir.join("bin/php.exe"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

pub fn find_php_fastcgi(dir: &Path) -> Option<PathBuf> {
    [
        dir.join("php-cgi.exe"),
        dir.join("php-fpm"),
        dir.join("sbin/php-fpm"),
        dir.join("bin/php-fpm"),
        dir.join("php-cgi"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Prefer a linked custom path; otherwise the sandbox `bin/php/{version}` tree.
pub fn resolve_php_home(
    app_root: &Path,
    version: &str,
    custom_paths: &std::collections::HashMap<String, String>,
) -> PathBuf {
    if let Some(custom) = custom_paths.get(version) {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    app_root.join("bin/php").join(version)
}

/// Inject library search paths so a bottle/Homebrew-style tree or a `lib/` sidecar works.
pub fn apply_macos_php_library_env(cmd: &mut Command, php_home: &Path) {
    #[cfg(target_os = "macos")]
    {
        let mut libs = Vec::new();
        let local_lib = php_home.join("lib");
        if local_lib.is_dir() {
            libs.push(local_lib);
        }
        // Homebrew keg layout: …/opt/php@7.4 → sibling deps live under …/opt/*/lib
        if let Some(opt_root) = php_home.parent() {
            if opt_root
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "opt")
                .unwrap_or(false)
            {
                libs.push(opt_root.to_path_buf());
            }
        }
        if libs.is_empty() {
            return;
        }
        let joined = std::env::join_paths(&libs).unwrap_or_default();
        let joined = joined.to_string_lossy();
        cmd.env("DYLD_LIBRARY_PATH", joined.as_ref());
        cmd.env("DYLD_FALLBACK_LIBRARY_PATH", joined.as_ref());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cmd, php_home);
    }
}

/// Validate a user-supplied PHP folder and return the first line of `php -v`.
pub async fn validate_custom_php_home(dir: &Path) -> Result<String> {
    if !dir.exists() {
        bail!("Directory not found: {}", dir.display());
    }
    let cli = find_php_cli(dir).ok_or_else(|| {
        anyhow::anyhow!("PHP CLI binary not found under {}. Expected php / bin/php.", dir.display())
    })?;
    let _fastcgi = find_php_fastcgi(dir).ok_or_else(|| {
        anyhow::anyhow!(
            "PHP FastCGI/FPM not found under {}. Expected php-fpm / sbin/php-fpm (or php-cgi.exe on Windows).",
            dir.display()
        )
    })?;

    let mut cmd = Command::new(&cli);
    cmd.arg("-v");
    apply_macos_php_library_env(&mut cmd, dir);
    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute {}", cli.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "PHP binary failed to run (wrong CPU arch or missing libraries). {}",
            stderr.trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next()
        .unwrap_or("PHP (unknown version)")
        .trim()
        .to_string();
    Ok(line)
}

/// Common Homebrew locations when the user already installed `php@X.Y`.
pub fn detect_homebrew_php(version: &str) -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(format!("/opt/homebrew/opt/php@{version}")),
        PathBuf::from(format!("/usr/local/opt/php@{version}")),
    ];
    candidates.into_iter().find(|p| php_home_is_valid(p))
}

//! OS-specific PHP availability copy — keeps the download manifest slim while
//! still telling the UI what users can Download vs Add+Link on this platform.

use serde::Serialize;

use crate::binary_manager::BinariesManifest;

#[derive(Debug, Clone, Serialize)]
pub struct PhpPlatformInfo {
    pub platform_id: String,
    pub platform_label: String,
    /// Version ids that have a one-click download in the manifest.
    pub downloadable: Vec<String>,
    /// Human-friendly patch versions for those downloads (aligned with `downloadable`).
    pub downloadable_versions: Vec<String>,
    /// Versions commonly used via Add version → Link folder (not in the slim manifest).
    pub linkable_examples: Vec<String>,
    pub link_hint: String,
    pub brew_formula_example: Option<String>,
    pub brew_path_example: Option<String>,
}

pub fn current_platform_id() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows_x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos_aarch64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos_x86_64"
    } else {
        "unknown"
    }
}

pub fn php_platform_info(manifest: &BinariesManifest) -> PhpPlatformInfo {
    let platform = manifest.for_current_platform();
    let mut downloadable: Vec<(String, String)> = platform
        .php
        .iter()
        .filter(|(_, spec)| {
            spec.url.as_ref().map(|u| !u.is_empty()).unwrap_or(false)
                && spec.status != "custom_only"
        })
        .map(|(id, spec)| (id.clone(), spec.version.clone()))
        .collect();
    downloadable.sort_by(|a, b| a.0.cmp(&b.0));

    let (ids, versions): (Vec<_>, Vec<_>) = downloadable.into_iter().unzip();

    match current_platform_id() {
        "windows_x64" => PhpPlatformInfo {
            platform_id: "windows_x64".into(),
            platform_label: "Windows x64".into(),
            downloadable: ids,
            downloadable_versions: versions,
            linkable_examples: vec![
                "5.6".into(),
                "7.0".into(),
                "7.1".into(),
                "7.2".into(),
                "7.3".into(),
                "8.0".into(),
                "8.1".into(),
            ],
            link_hint: "One-click downloads stay lean (7.4 + 8.2–8.4). For older Windows builds, Add a version label and Link any local PHP folder — or grab archives from windows.php.net yourself."
                .into(),
            brew_formula_example: None,
            brew_path_example: None,
        },
        "macos_aarch64" => PhpPlatformInfo {
            platform_id: "macos_aarch64".into(),
            platform_label: "macOS Apple Silicon".into(),
            downloadable: ids,
            downloadable_versions: versions,
            linkable_examples: vec![
                "5.6".into(),
                "7.0".into(),
                "7.1".into(),
                "7.2".into(),
                "7.3".into(),
                "7.4".into(),
                "8.0".into(),
                "8.1".into(),
            ],
            link_hint: "Portable downloads: 8.2–8.4 (static-php). Anything else — Add the version, then Link a Homebrew or custom folder."
                .into(),
            brew_formula_example: Some("brew tap shivammathur/php && brew install shivammathur/php/php@7.4".into()),
            brew_path_example: Some("/opt/homebrew/opt/php@7.4".into()),
        },
        "macos_x86_64" => PhpPlatformInfo {
            platform_id: "macos_x86_64".into(),
            platform_label: "macOS Intel".into(),
            downloadable: ids,
            downloadable_versions: versions,
            linkable_examples: vec![
                "5.6".into(),
                "7.0".into(),
                "7.1".into(),
                "7.2".into(),
                "7.3".into(),
                "7.4".into(),
                "8.0".into(),
                "8.1".into(),
            ],
            link_hint: "Portable downloads: 8.2–8.4 (static-php). For 5.6/7.x/8.0–8.1, Add a version and Link Homebrew (often under /usr/local/opt)."
                .into(),
            brew_formula_example: Some("brew tap shivammathur/php && brew install shivammathur/php/php@7.4".into()),
            brew_path_example: Some("/usr/local/opt/php@7.4".into()),
        },
        other => PhpPlatformInfo {
            platform_id: other.into(),
            platform_label: other.into(),
            downloadable: ids,
            downloadable_versions: versions,
            linkable_examples: vec!["7.4".into(), "8.1".into()],
            link_hint: "Add any version label, then Link a local PHP folder.".into(),
            brew_formula_example: None,
            brew_path_example: None,
        },
    }
}

//! Portable local server core for praews (zero system pollution).

pub mod binary_manager;
pub mod cli_env;
pub mod config_editor;
pub mod config_engine;
pub mod dns_server;
pub mod error;
pub mod orchestrator;
pub mod php_availability;
pub mod php_runtime;
pub mod portable;
pub mod sandbox;
pub mod ssl_manager;
pub mod vhost;

pub use binary_manager::{BinaryManager, ComponentSpec, DownloadProgress};
pub use cli_env::CliEnvManager;
pub use config_editor::{
    ConfigEditorManager, ConfigTarget, MySqlVisualSettings, NginxVisualSettings, PhpExtensionInfo,
    PhpVisualSettings,
};
pub use config_engine::{
    render_all_configs, render_all_vhosts, render_vhost, SandboxConfig, VhostConfig,
};
pub use dns_server::{
    ensure_macos_test_resolver, get_local_lan_ip, macos_test_resolver_ok, LocalDnsServer,
};
pub use error::CoreError;
pub use orchestrator::{ProcessOrchestrator, ServiceStatus, ServiceType};
pub use php_availability::{php_platform_info, PhpPlatformInfo};
pub use php_runtime::{
    apply_macos_php_library_env, detect_homebrew_php, php_home_is_valid, resolve_php_home,
    validate_custom_php_home,
};
pub use portable::{
    copy_dir_recursive, relocate_sandbox, verify_portable_layout, PortableCheck, RelocateMode,
};
pub use sandbox::{init_sandbox, resolve_app_root, seed_default_templates};
pub use ssl_manager::SslManager;
pub use vhost::{add_site, detect_public_path, load_state, SiteRecord};

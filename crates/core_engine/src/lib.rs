//! Portable local server core for praews (MAMP-style, zero system pollution).

pub mod binary_manager;
pub mod cli_env;
pub mod config_engine;
pub mod dns_server;
pub mod error;
pub mod orchestrator;
pub mod portable;
pub mod sandbox;
pub mod ssl_manager;
pub mod vhost;

pub use binary_manager::{BinaryManager, DownloadProgress};
pub use cli_env::CliEnvManager;
pub use config_engine::{render_all_configs, render_vhost, SandboxConfig, VhostConfig};
pub use dns_server::{get_local_lan_ip, LocalDnsServer};
pub use error::CoreError;
pub use orchestrator::{ProcessOrchestrator, ServiceStatus, ServiceType};
pub use portable::{verify_portable_layout, PortableCheck};
pub use sandbox::{init_sandbox, resolve_app_root, seed_default_templates};
pub use ssl_manager::SslManager;
pub use vhost::{add_site, detect_public_path, load_state, SiteRecord};

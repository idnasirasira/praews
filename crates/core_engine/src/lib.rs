//! Portable local server core for praews (MAMP-style, zero system pollution).

pub mod config_engine;
pub mod error;
pub mod orchestrator;
pub mod sandbox;

pub use config_engine::{render_all_configs, render_vhost, SandboxConfig};
pub use error::CoreError;
pub use orchestrator::{ProcessOrchestrator, ServiceStatus, ServiceType};
pub use sandbox::{init_sandbox, resolve_app_root, seed_default_templates};

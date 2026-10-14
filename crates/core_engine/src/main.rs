use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use core_engine::{
    add_site, init_sandbox, render_all_configs, resolve_app_root, BinaryManager, CliEnvManager,
    LocalDnsServer, ProcessOrchestrator, SandboxConfig, ServiceType, SslManager,
};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "praews", about = "Portable local web server (MAMP alternative) CLI")]
struct Cli {
    /// Override portable sandbox root (default: PRAEWS_ROOT or cwd)
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create sandbox directories and seed default templates
    Init,

    /// Render nginx.conf / php.ini / my.cnf from Tera templates
    Render {
        #[arg(long, default_value = "8.3")]
        php_version: String,
        #[arg(long, default_value_t = 8888)]
        http_port: u16,
        #[arg(long, default_value_t = 8443)]
        https_port: u16,
        #[arg(long, default_value_t = 9000)]
        php_port: u16,
        #[arg(long, default_value_t = 3306)]
        mysql_port: u16,
    },

    /// Print whether common service ports are free
    Ports {
        #[arg(long, default_value_t = 8888)]
        http_port: u16,
        #[arg(long, default_value_t = 9000)]
        php_port: u16,
        #[arg(long, default_value_t = 3306)]
        mysql_port: u16,
    },

    /// Download CA bundle into conf/ssl/cacert.pem
    EnsureCacert,

    /// Create local Root CA under conf/ssl/
    EnsureCa,

    /// Download & extract a component from the bundled platform manifest
    Install {
        #[arg(value_enum)]
        component: InstallComponent,
        #[arg(long, default_value = "8.3")]
        php_version: String,
    },

    /// Initialize MySQL/MariaDB system tables (no-op if already done)
    InitDb,

    /// Add a virtual host (.test) for a project folder
    AddSite {
        domain: String,
        #[arg(long)]
        path: PathBuf,
        #[arg(long, default_value_t = 8888)]
        http_port: u16,
        #[arg(long, default_value_t = 8443)]
        https_port: u16,
        #[arg(long, default_value_t = 9000)]
        php_port: u16,
    },

    /// Start local DNS resolver for *.test (default port 53535)
    Dns {
        #[arg(long, default_value_t = 53535)]
        port: u16,
    },

    /// Ensure composer shims (+ optionally download composer.phar)
    EnsureComposer {
        #[arg(long, default_value_t = true)]
        download: bool,
    },

    /// Print scoped export snippet for the active PHP version (eval-friendly)
    EnvSnippet {
        #[arg(long, default_value = "8.3")]
        php_version: String,
    },

    /// Open an OS terminal with portable PHP/MySQL/Composer PATH injected
    OpenTerminal {
        #[arg(long)]
        path: PathBuf,
        #[arg(long, default_value = "8.3")]
        php_version: String,
    },

    /// Show tracked service statuses
    Status,

    /// Start a service (requires portable binaries under bin/)
    Start {
        #[arg(value_enum)]
        service: CliService,
        #[arg(long, default_value = "8.3")]
        php_version: String,
        #[arg(long, default_value_t = 8888)]
        http_port: u16,
        #[arg(long, default_value_t = 9000)]
        php_port: u16,
        #[arg(long, default_value_t = 3306)]
        mysql_port: u16,
    },

    /// Stop a service
    Stop {
        #[arg(value_enum)]
        service: CliService,
    },

    /// Stop all managed services
    StopAll,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum CliService {
    Nginx,
    Php,
    Mysql,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum InstallComponent {
    Nginx,
    Php,
    Mariadb,
}

impl From<CliService> for ServiceType {
    fn from(value: CliService) -> Self {
        match value {
            CliService::Nginx => ServiceType::Nginx,
            CliService::Php => ServiceType::Php,
            CliService::Mysql => ServiceType::MySql,
        }
    }
}

fn resolve_root(cli_root: Option<PathBuf>) -> Result<PathBuf> {
    match cli_root {
        Some(path) => Ok(path),
        None => {
            if std::env::var_os("PRAEWS_ROOT").is_some() {
                resolve_app_root()
            } else {
                std::env::current_dir().context("cwd")
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let cli = Cli::parse();
    let app_root = resolve_root(cli.root)?;

    match cli.command {
        Commands::Init => {
            init_sandbox(&app_root)?;
            println!("sandbox ready at {}", app_root.display());
        }
        Commands::Render {
            php_version,
            http_port,
            https_port,
            php_port,
            mysql_port,
        } => {
            let config = SandboxConfig::new(&app_root, &php_version)
                .with_ports(http_port, php_port, mysql_port)
                .with_https_port(https_port);
            render_all_configs(&app_root, &config)?;
            println!("configs rendered under {}/conf", app_root.display());
        }
        Commands::Ports {
            http_port,
            php_port,
            mysql_port,
        } => {
            for (name, port) in [
                ("http", http_port),
                ("php-fastcgi", php_port),
                ("mysql", mysql_port),
            ] {
                let free = ProcessOrchestrator::is_port_available(port);
                println!(
                    "{name:>12} :{port} => {}",
                    if free { "available" } else { "IN USE" }
                );
            }
        }
        Commands::EnsureCacert => {
            init_sandbox(&app_root)?;
            let mgr = BinaryManager::new(&app_root)?;
            mgr.ensure_cacert_bundle().await?;
            println!("cacert ready at {}/conf/ssl/cacert.pem", app_root.display());
        }
        Commands::EnsureCa => {
            init_sandbox(&app_root)?;
            let ssl = SslManager::new(&app_root);
            ssl.ensure_root_ca()?;
            println!("Root CA at {}", ssl.root_ca_cert_path().display());
        }
        Commands::Install {
            component,
            php_version,
        } => {
            init_sandbox(&app_root)?;
            let mgr = BinaryManager::new(&app_root)?;
            let platform = mgr.manifest().for_current_platform();
            let (name, spec) = match component {
                InstallComponent::Nginx => ("nginx", &platform.nginx),
                InstallComponent::Mariadb => ("mariadb", &platform.mariadb),
                InstallComponent::Php => {
                    let spec = platform
                        .php
                        .get(&php_version)
                        .with_context(|| format!("PHP {php_version} not in manifest"))?;
                    ("php", spec)
                }
            };
            println!("installing {name} {} ...", spec.version);
            mgr.install_from_spec(name, spec, |p| {
                core_engine::binary_manager::print_progress(p);
            })
            .await?;
            eprintln!();
            println!("{name} installed under {}/{}", app_root.display(), spec.target_dir);
        }
        Commands::InitDb => {
            let mgr = BinaryManager::new(&app_root)?;
            mgr.initialize_mysql_datadir().await?;
            println!("MySQL datadir ready under {}/data/mysql", app_root.display());
        }
        Commands::AddSite {
            domain,
            path,
            http_port,
            https_port,
            php_port,
        } => {
            let site = add_site(&app_root, &domain, &path, http_port, https_port, php_port)?;
            println!(
                "site {} -> {} (public: {})",
                site.domain,
                site.project_path.display(),
                site.public_path.display()
            );
        }
        Commands::Dns { port } => {
            let mut dns = LocalDnsServer::new(port);
            dns.start().await?;
            println!("{}", dns.mobile_hint());
            println!("DNS listening on UDP :{port} — Ctrl+C to stop");
            tokio::signal::ctrl_c().await?;
            dns.stop();
        }
        Commands::EnsureComposer { download } => {
            init_sandbox(&app_root)?;
            let cli = CliEnvManager::new(&app_root);
            let shims = cli.ensure_composer_shims()?;
            if download {
                let client = reqwest::Client::builder()
                    .user_agent("praews/0.1")
                    .build()?;
                let phar = cli.ensure_composer_phar(&client).await?;
                println!("composer.phar at {}", phar.display());
            }
            println!("shims ready at {}", shims.display());
        }
        Commands::EnvSnippet { php_version } => {
            let cli = CliEnvManager::new(&app_root);
            println!("{}", cli.export_shell_snippet(&php_version)?);
        }
        Commands::OpenTerminal { path, php_version } => {
            init_sandbox(&app_root)?;
            let cli = CliEnvManager::new(&app_root);
            cli.open_project_terminal(&path, &php_version)?;
            println!("opened terminal in {} (PHP {php_version})", path.display());
        }
        Commands::Status => {
            let orch = ProcessOrchestrator::new(&app_root);
            let statuses = orch.get_all_statuses().await;
            for (svc, status) in statuses {
                println!("{svc:?}: {status:?}");
            }
        }
        Commands::Start {
            service,
            php_version,
            http_port,
            php_port,
            mysql_port,
        } => {
            let orch = ProcessOrchestrator::new(&app_root);
            let pid = match service {
                CliService::Nginx => orch.start_nginx(http_port).await?,
                CliService::Php => orch.start_php(&php_version, php_port).await?,
                CliService::Mysql => orch.start_mysql(mysql_port).await?,
            };
            println!("{service:?} started with pid {pid}");
            println!("Press Ctrl+C to stop this service");
            tokio::signal::ctrl_c().await?;
            orch.stop_service(service.into()).await?;
        }
        Commands::Stop { service } => {
            let orch = ProcessOrchestrator::new(&app_root);
            orch.stop_service(service.into()).await?;
            println!("{service:?} stop requested");
        }
        Commands::StopAll => {
            let orch = ProcessOrchestrator::new(&app_root);
            orch.stop_all().await?;
            println!("all services stop requested");
        }
    }

    Ok(())
}

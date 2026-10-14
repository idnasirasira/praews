use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use core_engine::{
    init_sandbox, render_all_configs, resolve_app_root, ProcessOrchestrator, SandboxConfig,
    ServiceType,
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

    /// Show tracked service statuses (requires prior start in this process)
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

impl From<CliService> for ServiceType {
    fn from(value: CliService) -> Self {
        match value {
            CliService::Nginx => ServiceType::Nginx,
            CliService::Php => ServiceType::Php,
            CliService::Mysql => ServiceType::MySql,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let cli = Cli::parse();
    let app_root = match cli.root {
        Some(path) => path,
        None => {
            // Prefer cwd for CLI testing so we don't write next to cargo target/
            if std::env::var_os("PRAEWS_ROOT").is_some() {
                resolve_app_root()?
            } else {
                std::env::current_dir().context("cwd")?
            }
        }
    };

    match cli.command {
        Commands::Init => {
            init_sandbox(&app_root)?;
            println!("sandbox ready at {}", app_root.display());
        }
        Commands::Render {
            php_version,
            http_port,
            php_port,
            mysql_port,
        } => {
            let config =
                SandboxConfig::new(&app_root, &php_version).with_ports(http_port, php_port, mysql_port);
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
            println!(
                "note: this CLI process owns the child; keep it running or use Stop/StopAll in the same session (GUI will own lifecycle later)"
            );
            // Keep process alive so kill_on_drop doesn't immediately reap the child during smoke tests.
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

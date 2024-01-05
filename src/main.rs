use anyhow::{Context, Result};
use clap::Parser;
use tokio::task::JoinSet;

pub(crate) mod package;
mod state;

use package::Package;
use state::State;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let state = state::State::load(&args.state_db)
        .await
        .context("Failed to load internal state")?;

    match args.command {
        Command::Install { pkgs } => {
            let mut set = JoinSet::new();

            for pkg in pkgs {
                let state = state.clone();
                set.spawn(async move { install_package(&state, &pkg).await });
            }

            let mut results = vec![];
            while let Some(result) = set.join_next().await {
                results.push(result?);
            }
            results.into_iter().collect::<Result<()>>()?;
        }
        Command::Uninstall { pkgs } => {
            let mut set = JoinSet::new();

            for pkg in pkgs {
                let state = state.clone();
                set.spawn(async move { uninstall_package(&state, &pkg).await });
            }

            let mut results = vec![];
            while let Some(result) = set.join_next().await {
                results.push(result?);
            }
            results.into_iter().collect::<Result<()>>()?;
        }
        Command::List => list_packages(&state).await?,
        Command::Registry(cmd) => match cmd {
            RegistryCommand::Add { uris } => {
                let mut set = JoinSet::new();
                for uri in uris {
                    let state = state.clone();
                    set.spawn(async move { add_registry(&state, &uri).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    results.push(result?);
                }
                results.into_iter().collect::<Result<()>>()?;
            }
            RegistryCommand::Remove { uris } => {
                let mut set = JoinSet::new();
                for uri in uris {
                    let state = state.clone();
                    set.spawn(async move { remove_registry(&state, &uri).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    results.push(result?);
                }
                results.into_iter().collect::<Result<()>>()?;
            }
            RegistryCommand::List => {
                list_registries(&state).await?;
            }
        },
    }

    Ok(())
}

#[derive(Parser, Debug)]
struct Cli {
    /// Command to run
    #[command(subcommand)]
    command: Command,

    /// Path to the internal state database
    #[arg(short, long, default_value = "~/.config/pkg/state.db")]
    state_db: String,
}

#[derive(Parser, Debug)]
enum Command {
    /// Install one or more packages
    #[command(arg_required_else_help = true)]
    Install {
        /// Packages to install
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// Uninstall one or more packages
    #[command(arg_required_else_help = true)]
    Uninstall {
        /// Packages to uninstall
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// List all installed packages
    List,

    /// Manage registries
    #[command(subcommand)]
    Registry(RegistryCommand),
}

#[derive(Parser, Debug)]
enum RegistryCommand {
    /// Add one or more registries
    #[command(arg_required_else_help = true)]
    Add {
        /// Registry to add
        #[arg(required = true)]
        uris: Vec<String>,
    },

    /// Remove one or more registries
    #[command(arg_required_else_help = true)]
    Remove {
        /// Registry to add
        #[arg(required = true)]
        uris: Vec<String>,
    },

    /// List all registries
    List,
}

/// Installs a package.
async fn install_package(state: &State, pkg: &str) -> Result<()> {
    let pkg = pkg.parse().context("failed to parse package name")?;

    // TODO: Deal with rollbacks for failed installs.
    state
        .add_installed_package(&pkg)
        .await
        .context("failed to register installed package")?;

    println!("Installed {pkg}");
    Ok(())
}

/// Uninstalls a package.
async fn uninstall_package(state: &State, pkg: &str) -> Result<()> {
    let mut pkg: Package = pkg.parse().context("failed to parse package name")?;

    state.resolve_installed_version(&mut pkg).await?;

    state
        .remove_installed_package(&pkg)
        .await
        .context("failed to deregister installed package")?;

    println!("Uninstalled {pkg}");
    Ok(())
}

/// Lists all installed packages.
async fn list_packages(state: &State) -> Result<()> {
    let packages = state.installed_packages().await?;

    for pkg in packages {
        println!("{}", pkg);
    }

    Ok(())
}

/// Adds a registry.
async fn add_registry(state: &State, uri: &str) -> Result<()> {
    state.add_registry(uri).await?;

    println!("Added registry {}", uri);
    Ok(())
}

/// Removes a registry.
async fn remove_registry(state: &State, uri: &str) -> Result<()> {
    state.remove_registry(uri).await?;

    println!("Removed registry {}", uri);
    Ok(())
}

/// Lists all registries.
async fn list_registries(state: &State) -> Result<()> {
    let registries = state.registries().await?;

    for registry in registries {
        println!("{}", registry);
    }

    Ok(())
}

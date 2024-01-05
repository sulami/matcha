use anyhow::{anyhow, Context, Result};
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

    if state.is_package_installed(&pkg).await? {
        return Err(anyhow!("package {} is already installed", pkg));
    }

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

    state.resolve_installed_package_version(&mut pkg).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_install_package() {
        let state = State::load(":memory:").await.unwrap();
        let pkg = "foo@1.2.3";

        install_package(&state, pkg).await.unwrap();
        assert!(state
            .is_package_installed(&pkg.parse().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_install_package_refuses_if_package_is_already_installed() {
        let state = State::load(":memory:").await.unwrap();
        let pkg = "foo@1.2.3";

        install_package(&state, pkg).await.unwrap();
        let result = install_package(&state, pkg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_uninstall_package() {
        let state = State::load(":memory:").await.unwrap();
        let pkg = "foo@1.2.3";

        install_package(&state, pkg).await.unwrap();
        uninstall_package(&state, pkg).await.unwrap();
        assert!(!state
            .is_package_installed(&pkg.parse().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_uninstall_package_refuses_if_package_is_not_installed() {
        let state = State::load(":memory:").await.unwrap();
        let pkg = "foo@1.2.3";

        let result = uninstall_package(&state, pkg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_packages() {
        let state = State::load(":memory:").await.unwrap();
        let pkg = "foo@1.2.3";

        install_package(&state, pkg).await.unwrap();
        list_packages(&state).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_packages_empty() {
        let state = State::load(":memory:").await.unwrap();
        list_packages(&state).await.unwrap();
    }

    #[tokio::test]
    async fn test_add_registry() {
        let state = State::load(":memory:").await.unwrap();
        let uri = "https://example.invalid";

        add_registry(&state, uri).await.unwrap();
        assert!(state.registries().await.unwrap().contains(&uri.to_string()));
    }

    #[tokio::test]
    async fn test_add_registry_refuses_if_registry_is_already_added() {
        let state = State::load(":memory:").await.unwrap();
        let uri = "https://example.invalid";

        add_registry(&state, uri).await.unwrap();
        let result = add_registry(&state, uri).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_registry() {
        let state = State::load(":memory:").await.unwrap();
        let uri = "https://example.invalid";

        add_registry(&state, uri).await.unwrap();
        remove_registry(&state, uri).await.unwrap();
        assert!(!state.registries().await.unwrap().contains(&uri.to_string()));
    }

    #[tokio::test]
    async fn test_remove_registry_refuses_if_registry_is_not_added() {
        let state = State::load(":memory:").await.unwrap();
        let uri = "https://example.invalid";

        let result = remove_registry(&state, uri).await;
        assert!(result.is_err());
    }
}

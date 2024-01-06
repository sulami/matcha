use anyhow::{anyhow, Context, Result};
use clap::Parser;
use tokio::task::JoinSet;

pub(crate) mod download;
pub(crate) mod manifest;
pub(crate) mod package;
pub(crate) mod progress;
pub(crate) mod registry;
pub(crate) mod state;

use package::Package;
use progress::create_progress_bar;
use registry::{DefaultFetcher, Fetcher, Registry};
use state::State;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let state = state::State::load(&args.state_db)
        .await
        .context("Failed to load internal state")?;

    match args.command {
        Command::Install { pkgs } => {
            ensure_registries_are_current(&state, &DefaultFetcher, false).await?;

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
        Command::Search { query } => {
            ensure_registries_are_current(&state, &DefaultFetcher, false).await?;

            search_packages(&state, &query).await?;
        }
        Command::Fetch => {
            ensure_registries_are_current(&state, &DefaultFetcher, true).await?;
        }
        Command::Registry(cmd) => match cmd {
            RegistryCommand::Add { uri } => {
                add_registry(&state, &uri, &DefaultFetcher).await?;
            }
            RegistryCommand::Remove { name } => {
                remove_registry(&state, &name).await?;
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
    #[command(arg_required_else_help = true, alias = "i", alias = "add")]
    Install {
        /// Packages to install
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// Uninstall one or more packages
    #[command(
        arg_required_else_help = true,
        alias = "u",
        alias = "remove",
        alias = "rm"
    )]
    Uninstall {
        /// Packages to uninstall
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// List all installed packages
    #[command(alias = "ls")]
    List,

    /// Search for a package
    #[command(arg_required_else_help = true, alias = "s", alias = "find")]
    Search {
        /// Search query
        query: String,
    },

    /// Fetch all registries
    Fetch,

    /// Manage registries
    #[command(subcommand, alias = "r", alias = "reg")]
    Registry(RegistryCommand),
}

#[derive(Parser, Debug)]
enum RegistryCommand {
    /// Add a package registry
    #[command(arg_required_else_help = true)]
    Add {
        /// Registry to add
        uri: String,
    },

    /// Remove a package registry
    #[command(arg_required_else_help = true, alias = "rm")]
    Remove {
        /// Registry to remove
        name: String,
    },

    /// List all registries
    #[command(alias = "ls")]
    List,
}

/// Installs a package.
async fn install_package(state: &State, pkg: &str) -> Result<()> {
    let mut pkg = pkg.parse().context("failed to parse package name")?;

    if state.is_package_installed(&pkg).await? {
        return Err(anyhow!("package {} is already installed", pkg));
    }

    pkg.resolve_known_version(state)
        .await
        .context("failed to resolve package version")?;

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
    pkg.resolve_version(state)
        .await
        .context("failed to resolve package version")?;

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
async fn add_registry(state: &State, uri: &str, fetcher: &impl Fetcher) -> Result<()> {
    let mut registry = Registry::new(uri);
    registry.initialize(fetcher).await?;
    state.add_registry(&registry).await?;

    println!("Added registry {}", registry);
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

/// Ensures all registries are up to date by potentially refetching them.
///
/// Supply `force` to force a refetch of all registries.
async fn ensure_registries_are_current(
    state: &State,
    fetcher: &(impl Fetcher + 'static),
    force: bool,
) -> Result<()> {
    let registries = state.registries().await?;

    let pb = create_progress_bar("Fetching registries", registries.len() as u64);
    let mut set = JoinSet::new();

    for mut registry in registries {
        if force || registry.should_update() {
            let state = state.clone();
            let fetcher = fetcher.clone();
            set.spawn(async move { registry.update(&state, &fetcher).await });
        }
    }

    let mut results = vec![];
    while let Some(result) = set.join_next().await {
        results.push(result?);
        pb.inc(1);
    }

    results
        .into_iter()
        .collect::<Result<()>>()
        .context("failed to update registries")?;

    Ok(())
}

/// Searches for a package.
async fn search_packages(state: &State, query: &str) -> Result<()> {
    let packages = state.search_known_packages(query).await?;

    for pkg in packages {
        println!("{}", pkg);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::registry::MockFetcher;

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

        add_registry(&state, uri, &MockFetcher::default())
            .await
            .unwrap();
        assert!(state
            .registries()
            .await
            .unwrap()
            .iter()
            .any(|r| r.uri.to_string() == uri));
    }

    #[tokio::test]
    async fn test_add_registry_refuses_if_registry_is_already_added() {
        let state = State::load(":memory:").await.unwrap();
        let uri = "https://example.invalid";

        add_registry(&state, uri, &MockFetcher::default())
            .await
            .unwrap();
        let result = add_registry(&state, uri, &MockFetcher::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_registry() {
        let state = State::load(":memory:").await.unwrap();

        add_registry(&state, "https://example.invalid", &MockFetcher::default())
            .await
            .unwrap();
        remove_registry(&state, "test").await.unwrap();
        assert!(!state.registry_exists_by_name("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_remove_registry_refuses_if_registry_is_not_added() {
        let state = State::load(":memory:").await.unwrap();
        let result = remove_registry(&state, "foo").await;
        assert!(result.is_err());
    }
}

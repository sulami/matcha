use std::{env::var, path::PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use once_cell::sync::OnceCell;
use tokio::task::JoinSet;

pub(crate) mod download;
pub(crate) mod manifest;
pub(crate) mod package;
pub(crate) mod registry;
pub(crate) mod state;
pub(crate) mod ui;
pub(crate) mod workspace;

use package::{InstalledPackageSpec, KnownPackageSpec, PackageRequest};
use registry::{DefaultFetcher, Fetcher, Registry};
use state::State;
use ui::create_progress_bar;
use workspace::Workspace;

/// The root directory that holds all the workspaces.
static WORKSPACE_DIRECTORY: OnceCell<PathBuf> = OnceCell::new();

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let state = state::State::load(&args.state_db)
        .await
        .context("Failed to load internal state")?;

    WORKSPACE_DIRECTORY.set(args.workspace_directory).unwrap();

    match args.command {
        Command::Package(cmd) => match cmd {
            PackageCommand::Install { pkgs, workspace } => {
                ensure_registries_are_current(&state, &DefaultFetcher, false).await?;

                let pb = create_progress_bar("Installing packages", pkgs.len() as u64);
                let mut set = JoinSet::new();

                for pkg in pkgs {
                    let state = state.clone();
                    let workspace = workspace.clone();
                    set.spawn(async move { install_package(&state, &pkg, workspace).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    pb.inc(1);
                    results.push(result?);
                }
                pb.finish_and_clear();
                let output = results.into_iter().collect::<Result<Vec<String>>>()?;
                for line in output {
                    println!("{}", line);
                }
            }
            PackageCommand::Update {
                mut pkgs,
                workspace,
            } => {
                ensure_registries_are_current(&state, &DefaultFetcher, false).await?;

                if pkgs.is_empty() {
                    pkgs = state
                        .installed_packages(&Workspace::default())
                        .await?
                        .into_iter()
                        .map(|pkg| pkg.name)
                        .collect();
                }

                let pb = create_progress_bar("Updating packages", pkgs.len() as u64);
                let mut set = JoinSet::new();

                for pkg in pkgs {
                    let state = state.clone();
                    let workspace = workspace.clone();
                    set.spawn(async move { update_package(&state, &pkg, workspace).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    pb.inc(1);
                    results.push(result?);
                }
                pb.finish_and_clear();
                let output = results
                    .into_iter()
                    .collect::<Result<Vec<Option<String>>>>()?;
                output
                    .into_iter()
                    .flatten()
                    .for_each(|line| println!("{}", line));
            }
            PackageCommand::Remove { pkgs, workspace } => {
                let pb = create_progress_bar("Removing packages", pkgs.len() as u64);
                let mut set = JoinSet::new();

                for pkg in pkgs {
                    let state = state.clone();
                    let workspace = workspace.clone();
                    set.spawn(async move { uninstall_package(&state, &pkg, workspace).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    pb.inc(1);
                    results.push(result?);
                }
                pb.finish_and_clear();
                let output = results.into_iter().collect::<Result<Vec<String>>>()?;
                for line in output {
                    println!("{}", line);
                }
            }
            PackageCommand::List { workspace } => list_packages(&state, workspace).await?,
            PackageCommand::Search {
                query,
                all_versions,
            } => {
                ensure_registries_are_current(&state, &DefaultFetcher, false).await?;

                search_packages(&state, &query, all_versions).await?;
            }
        },
        Command::Workspace(cmd) => match cmd {
            WorkspaceCommand::Add { workspace } => {
                add_workspace(&state, &workspace).await?;
            }
            WorkspaceCommand::Remove { workspace } => {
                remove_workspace(&state, &workspace).await?;
            }
            WorkspaceCommand::List => {
                list_workspaces(&state).await?;
            }
            WorkspaceCommand::Shell { workspace } => {
                workspace_shell(&state, &workspace).await?;
            }
        },
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
            RegistryCommand::Fetch => {
                ensure_registries_are_current(&state, &DefaultFetcher, true).await?;
            }
        },
    }

    Ok(())
}

/// A peaceful package manager
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    /// Command to run
    #[command(subcommand)]
    command: Command,

    /// Path to the internal state database
    #[arg(
        long,
        env = "MATCHA_STATE_DB",
        default_value = "~/.config/matcha/state.db"
    )]
    state_db: String,

    /// Path to the packge directory
    #[arg(
        long,
        env = "MATCHA_WORKSPACE_DIR",
        default_value = "~/.config/matcha/workspaces"
    )]
    workspace_directory: PathBuf,
}

#[derive(Parser, Debug)]
enum Command {
    /// Manage packages (alias: pkg, p)
    #[command(subcommand, arg_required_else_help = true, alias = "pkg", alias = "p")]
    Package(PackageCommand),

    /// Manage workspaces (alias: ws, w)
    #[command(subcommand, arg_required_else_help = true, alias = "ws", alias = "w")]
    Workspace(WorkspaceCommand),

    /// Manage registries (alias: reg, r)
    #[command(subcommand, arg_required_else_help = true, alias = "reg", alias = "r")]
    Registry(RegistryCommand),
}

#[derive(Parser, Debug)]
enum PackageCommand {
    /// Install one or more packages (alias: i)
    #[command(arg_required_else_help = true, alias = "i")]
    Install {
        /// Workspace to use
        #[arg(short, long)]
        workspace: Option<String>,

        /// Packages to install
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// Update all or select packages (alias: u)
    #[command(alias = "u")]
    Update {
        /// Workspace to use
        #[arg(short, long)]
        workspace: Option<String>,

        /// Select packages to update
        pkgs: Vec<String>,
    },

    /// Remove one or more packages (alias: rm)
    #[command(arg_required_else_help = true, alias = "rm")]
    Remove {
        /// Workspace to use
        #[arg(short, long)]
        workspace: Option<String>,

        /// Packages to uninstall
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// List all installed packages (alias: ls)
    #[command(alias = "ls")]
    List {
        /// Workspace to use
        #[arg(short, long)]
        workspace: Option<String>,
    },

    /// Search for a package (alias: s)
    #[command(arg_required_else_help = true, alias = "s")]
    Search {
        /// Search query
        query: String,

        /// Return all versions instead of just the latest
        #[arg(long)]
        all_versions: bool,
    },
}

#[derive(Parser, Debug)]
enum WorkspaceCommand {
    /// Add a workspace (alias: a)
    #[command(arg_required_else_help = true, alias = "a")]
    Add { workspace: String },

    /// Remove a workspace (alias: rm)
    #[command(arg_required_else_help = true, alias = "rm")]
    Remove { workspace: String },

    /// List all workspaces (alias: ls)
    #[command(alias = "ls")]
    List,

    /// Run a shell in the context of a workspace (alias: sh)
    #[command(alias = "sh")]
    Shell { workspace: String },
}

#[derive(Parser, Debug)]
enum RegistryCommand {
    /// Add a package registry (alias: a)
    #[command(arg_required_else_help = true, alias = "a")]
    Add {
        /// Registry to add
        uri: String,
    },

    /// Remove a package registry (alias: rm)
    #[command(arg_required_else_help = true, alias = "rm")]
    Remove {
        /// Registry to remove
        name: String,
    },

    /// List all registries (alias: ls)
    #[command(alias = "ls")]
    List,

    /// Fetch all registries
    Fetch,
}

/// Gets a workspace by name, if supplied. Otherwise defaults to the global workspace.
///
/// Also ensures the directory actually exists.
async fn get_create_workspace(state: &State, name: Option<String>) -> Result<Workspace> {
    let ws = if let Some(name) = name {
        if let Some(ws) = state
            .get_workspace(&name)
            .await
            .context("failed to retrieve workspace")?
        {
            ws
        } else {
            return Err(anyhow!("workspace {} does not exist", name));
        }
    } else {
        Workspace::default()
    };

    ws.ensure_exists().await?;

    Ok(ws)
}

/// Installs a package.
async fn install_package(state: &State, pkg: &str, workspace: Option<String>) -> Result<String> {
    let pkg_req: PackageRequest = pkg.parse().context("failed to parse package name")?;
    let pkg_spec: KnownPackageSpec = pkg_req
        .resolve_known_version(state)
        .await
        .context("failed to resolve package version")?;

    let workspace = get_create_workspace(state, workspace).await?;

    if state.is_package_installed(&pkg_spec, &workspace).await? {
        return Err(anyhow!("package {} is already installed", pkg));
    }

    let pkg = state.get_package(&pkg_spec).await?;
    pkg.build(&workspace).await?;

    state
        .add_installed_package(&pkg_spec, &Workspace::default())
        .await
        .context("failed to register installed package")?;

    Ok(format!("Installed {pkg_spec}"))
}

/// Updates a package.
async fn update_package(
    state: &State,
    pkg: &str,
    workspace: Option<String>,
) -> Result<Option<String>> {
    let pkg_req: PackageRequest = pkg.parse().context("failed to parse package name")?;
    let workspace = get_create_workspace(state, workspace).await?;
    let pkg_spec: InstalledPackageSpec = pkg_req
        .resolve_installed_version(state, &workspace)
        .await
        .context("failed to resolve package version")?;

    if let Some(new_version) = pkg_spec.available_update(state).await? {
        // install update
        // state
        //     .update_installed_package(&pkg, &new_version)
        //     .await
        //     .context("failed to update installed package")?;
        Ok(Some(format!("Updated {pkg_spec} to {new_version}")))
    } else {
        Ok(None)
    }
}

/// Uninstalls a package.
async fn uninstall_package(state: &State, pkg: &str, workspace: Option<String>) -> Result<String> {
    let pkg_req: PackageRequest = pkg.parse().context("failed to parse package name")?;
    let workspace = get_create_workspace(state, workspace).await?;
    let pkg_spec: InstalledPackageSpec = pkg_req
        .resolve_installed_version(state, &workspace)
        .await
        .context("failed to resolve package version")?;

    state
        .remove_installed_package(&pkg_spec, &Workspace::default())
        .await
        .context("failed to deregister installed package")?;

    Ok(format!("Uninstalled {pkg_spec}"))
}

/// Lists all installed packages.
async fn list_packages(state: &State, workspace: Option<String>) -> Result<()> {
    let workspace = get_create_workspace(state, workspace).await?;
    let packages = state.installed_packages(&workspace).await?;

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

    eprintln!("Added registry {}", registry);
    Ok(())
}

/// Removes a registry.
async fn remove_registry(state: &State, uri: &str) -> Result<()> {
    state.remove_registry(uri).await?;

    eprintln!("Removed registry {}", uri);
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
        pb.inc(1);
        results.push(result?);
    }

    pb.finish_and_clear();
    results
        .into_iter()
        .collect::<Result<()>>()
        .context("failed to update registries")?;

    Ok(())
}

/// Searches for a package.
async fn search_packages(state: &State, query: &str, all_versions: bool) -> Result<()> {
    let packages = if all_versions {
        state.search_known_packages(query).await?
    } else {
        state.search_known_packages_latest_only(query).await?
    };

    for pkg in packages {
        println!("{}", pkg);
    }

    Ok(())
}

/// Adds a workspace.
async fn add_workspace(state: &State, name: &str) -> Result<()> {
    if name
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
    {
        return Err(anyhow!("workspace names can contain [a-zA-Z0-9-_] only"));
    }

    if state.get_workspace(name).await?.is_some() {
        return Err(anyhow!("workspace {} already exists", name));
    }

    state.add_workspace(&Workspace::new(name)).await?;
    Ok(())
}

/// Removes a workspace.
async fn remove_workspace(state: &State, name: &str) -> Result<()> {
    if name == "global" {
        return Err(anyhow!("cannot remove global workspace"));
    }
    if state.get_workspace(name).await?.is_none() {
        return Err(anyhow!("workspace {} does not exist", name));
    }
    state.remove_workspace(name).await?;
    Ok(())
}

/// Lists all workspaces.
async fn list_workspaces(state: &State) -> Result<()> {
    let workspaces = state.workspaces().await?;

    for workspace in workspaces {
        println!("{}", workspace);
    }

    Ok(())
}

/// Runs a shell in the context of a workspace.
async fn workspace_shell(state: &State, workspace: &str) -> Result<()> {
    if state.get_workspace(workspace).await?.is_none() {
        return Err(anyhow!("workspace {} does not exist", workspace));
    }

    let system_shell = var("SHELL").unwrap_or_else(|_| "zsh".to_string());
    tokio::process::Command::new(system_shell)
        // TODO Patch the $PATH to include the workspace's bin directory.
        .env("PKG_WORKSPACE", workspace)
        .spawn()
        .context("failed to run workspace shell")?
        .wait()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::{tempdir, TempDir};

    use super::*;

    use crate::registry::MockFetcher;

    /// Convenience function to setup the default test state.
    async fn setup_state_with_registry() -> Result<State> {
        let state = State::load(":memory:").await?;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&MockFetcher::default()).await?;
        state.add_registry(&registry).await?;
        ensure_registries_are_current(&state, &MockFetcher::default(), false).await?;
        Ok(state)
    }

    /// Creates a temporary workspace directory and sets it as the global workspace directory.
    ///
    /// Ensure you keep this in scope, as the directory will be deleted when it is dropped.
    fn temp_workspace_directory() -> Result<TempDir> {
        let dir = tempdir().context("failed to create temporary workspace directory")?;
        WORKSPACE_DIRECTORY
            .set(dir.path().to_path_buf())
            .expect("double init for WORKSPACE_DIRECTORY");
        Ok(dir)
    }

    #[tokio::test]
    async fn test_install_package() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        let pkg: PackageRequest = "test-package@0.1.0".parse().unwrap();
        let pkg: KnownPackageSpec = pkg.resolve_known_version(&state).await.unwrap();

        install_package(&state, &pkg.name, None).await.unwrap();
        assert!(state
            .is_package_installed(&pkg, &Workspace::default())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_install_package_refuses_if_package_is_already_installed() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        let pkg = "test-package@0.1.0";

        install_package(&state, pkg, None).await.unwrap();
        let result = install_package(&state, pkg, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_uninstall_package() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        let pkg: PackageRequest = "test-package@0.1.0".parse().unwrap();
        let pkg: KnownPackageSpec = pkg.resolve_known_version(&state).await.unwrap();

        install_package(&state, &pkg.name, None).await.unwrap();
        uninstall_package(&state, &pkg.name, None).await.unwrap();
        assert!(!state
            .is_package_installed(&pkg, &Workspace::default())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_uninstall_package_refuses_if_package_is_not_installed() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        let pkg = "test-package@0.1.0";

        let result = uninstall_package(&state, pkg, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_packages() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        let pkg = "test-package@0.1.0";

        install_package(&state, pkg, None).await.unwrap();
        list_packages(&state, None).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_packages_empty() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        list_packages(&state, None).await.unwrap();
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

    #[tokio::test]
    async fn test_add_workspace() {
        let state = State::load(":memory:").await.unwrap();
        let name = "test";

        add_workspace(&state, name).await.unwrap();
        assert!(state
            .workspaces()
            .await
            .unwrap()
            .iter()
            .any(|w| w.name == name));
    }

    #[tokio::test]
    async fn test_add_workspace_refuses_same_name_twice() {
        let state = State::load(":memory:").await.unwrap();
        let name = "test";

        add_workspace(&state, name).await.unwrap();
        let result = add_workspace(&state, name).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_workspace_refuses_global() {
        let state = State::load(":memory:").await.unwrap();
        let name = "global";

        let result = remove_workspace(&state, name).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_remove_workspace_refuses_nonexistent() {
        let state = State::load(":memory:").await.unwrap();
        let name = "test";

        let result = remove_workspace(&state, name).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_workspaces() {
        let state = State::load(":memory:").await.unwrap();
        let name = "test";

        add_workspace(&state, name).await.unwrap();
        list_workspaces(&state).await.unwrap();
    }

    #[tokio::test]
    async fn test_remove_workspace_with_packages() {
        let state = setup_state_with_registry().await.unwrap();
        let _workspace_dir = temp_workspace_directory().unwrap();
        let workspace = Workspace::new("test");

        add_workspace(&state, &workspace.name).await.unwrap();
        install_package(&state, "test-package@0.1.0", Some(workspace.name.clone()))
            .await
            .unwrap();
        remove_workspace(&state, "test").await.unwrap();
        assert!(!state
            .is_package_installed(
                &"test-package@0.1.0"
                    .parse::<PackageRequest>()
                    .unwrap()
                    .resolve_known_version(&state)
                    .await
                    .unwrap(),
                &workspace
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_add_workspace_rejects_invalid_names() {
        let state = State::load(":memory:").await.unwrap();
        let name = "test!";

        let result = add_workspace(&state, name).await;
        assert!(result.is_err());
    }
}

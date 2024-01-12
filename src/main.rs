use std::{env::var, ops::Deref, path::PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use manifest::InstallLog;
use once_cell::sync::OnceCell;
use shellexpand::tilde;
use tokio::task::JoinSet;

pub(crate) mod download;
pub(crate) mod manifest;
pub(crate) mod package;
pub(crate) mod registry;
pub(crate) mod state;
pub(crate) mod ui;
pub(crate) mod util;
pub(crate) mod workspace;

use package::{KnownPackageSpec, PackageRequest, WorkspacePackageSpec};
use registry::{DefaultFetcher, Fetcher, Registry};
use state::State;
use ui::create_progress_bar;
use util::is_file_system_safe;
use workspace::Workspace;

/// The root directory that holds all the workspaces.
static WORKSPACE_ROOT: OnceCell<PathBuf> = OnceCell::new();

/// The root directory that holds all installed packages.
static PACKAGE_ROOT: OnceCell<PathBuf> = OnceCell::new();

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let state = state::State::load(&args.state_db)
        .await
        .context("Failed to load internal state")?;

    WORKSPACE_ROOT
        .set(PathBuf::from(
            tilde(&args.workspace_root.to_string_lossy()).deref(),
        ))
        .expect("double initialization of WORKSPACE_ROOT");
    PACKAGE_ROOT
        .set(PathBuf::from(
            tilde(&args.package_root.to_string_lossy()).deref(),
        ))
        .expect("double initialization of PACKAGE_ROOT");

    match args.command {
        Command::Package(cmd) => match cmd {
            PackageCommand::Install { pkgs, workspace } => {
                let workspace = get_create_workspace(&state, &workspace).await?;
                ensure_registries_are_current(&state, &DefaultFetcher, false).await?;

                let pb = create_progress_bar("Installing packages", pkgs.len() as u64);
                let mut set = JoinSet::new();

                for pkg in pkgs {
                    let state = state.clone();
                    let workspace = workspace.clone();
                    set.spawn(async move { install_package(&state, &pkg, &workspace).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    pb.inc(1);
                    results.push(result?);
                }
                pb.finish_and_clear();
                let logs = results.into_iter().collect::<Result<Vec<InstallLog>>>()?;
                for log in logs {
                    if log.is_success() {
                        println!("Installed {}", log.package_name);
                    } else {
                        println!(
                            "Failed to install {}, build exited with code {}\nSTDOUT:\n{}STDERR:\n{}",
                            log.package_name, log.exit_code, log.stdout, log.stderr
                        );
                    }
                }

                check_path_for_workspace(&workspace);
            }
            PackageCommand::Update {
                mut pkgs,
                workspace,
            } => {
                ensure_registries_are_current(&state, &DefaultFetcher, false).await?;
                let workspace = get_create_workspace(&state, &workspace).await?;

                if pkgs.is_empty() {
                    pkgs = state
                        .workspace_packages(&workspace)
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
                    set.spawn(async move { update_package(&state, &pkg, &workspace).await });
                }

                let mut results = vec![];
                while let Some(result) = set.join_next().await {
                    pb.inc(1);
                    results.push(result?);
                }
                pb.finish_and_clear();
                let logs = results
                    .into_iter()
                    .collect::<Result<Vec<Option<InstallLog>>>>()?;
                for log in logs {
                    let Some(log) = log else {
                        continue;
                    };
                    if log.is_success() {
                        println!("Installed {}", log.package_name);
                    } else {
                        println!(
                            "Failed to install {}, build exited with code {}\nSTDOUT:\n{}STDERR:\n{}",
                            log.package_name, log.exit_code, log.stdout, log.stderr
                        );
                    }
                }
            }
            PackageCommand::Remove { pkgs, workspace } => {
                let workspace = get_create_workspace(&state, &workspace).await?;
                let pb = create_progress_bar("Removing packages", pkgs.len() as u64);
                let mut set = JoinSet::new();

                for pkg in pkgs {
                    let state = state.clone();
                    let workspace = workspace.clone();
                    set.spawn(async move { uninstall_package(&state, &pkg, &workspace).await });
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
            PackageCommand::List { workspace } => {
                let workspace = get_create_workspace(&state, &workspace).await?;
                list_packages(&state, &workspace).await?;
            }
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
            RegistryCommand::Remove { uri } => {
                remove_registry(&state, &uri).await?;
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
        default_value = "~/.local/matcha/state.db"
    )]
    state_db: String,

    /// Path to the workspace directory
    #[arg(
        long,
        env = "MATCHA_WORKSPACE_ROOT",
        default_value = "~/.local/matcha/workspaces"
    )]
    workspace_root: PathBuf,

    /// Path to the packge directory
    #[arg(
        long,
        env = "MATCHA_WORKSPACE_ROOT",
        default_value = "~/.local/matcha/packages"
    )]
    package_root: PathBuf,
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
        #[arg(short, long, env = "MATCHA_WORKSPACE", default_value = "global")]
        workspace: String,

        /// Packages to install
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// Update all or select packages (alias: u)
    #[command(alias = "u")]
    Update {
        /// Workspace to use
        #[arg(short, long, env = "MATCHA_WORKSPACE", default_value = "global")]
        workspace: String,

        /// Select packages to update
        pkgs: Vec<String>,
    },

    /// Remove one or more packages (alias: rm)
    #[command(arg_required_else_help = true, alias = "rm")]
    Remove {
        /// Workspace to use
        #[arg(short, long, env = "MATCHA_WORKSPACE", default_value = "global")]
        workspace: String,

        /// Packages to uninstall
        #[arg(required = true)]
        pkgs: Vec<String>,
    },

    /// List all installed packages (alias: ls)
    #[command(alias = "ls")]
    List {
        /// Workspace to use
        #[arg(short, long, env = "MATCHA_WORKSPACE", default_value = "global")]
        workspace: String,
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
        uri: String,
    },

    /// List all registries (alias: ls)
    #[command(alias = "ls")]
    List,

    /// Fetch all registries
    Fetch,
}

/// Returns the current value of $PATH.
fn current_path() -> String {
    var("PATH").unwrap_or_else(|_| "".to_string())
}

/// Checks if the current workspace bin dir is in $PATH, and emit a message if it isn't.
fn check_path_for_workspace(workspace: &Workspace) {
    let path = current_path();
    let bin_dir = workspace.bin_directory().unwrap();
    if !path.split(':').any(|p| p == bin_dir.to_str().unwrap()) {
        eprintln!(
            r"Warning: the workspace bin directory is not in $PATH.
Add this to your shell's configuration file:

export PATH={0}:$PATH",
            bin_dir.display()
        );
    }
}

/// Gets a workspace by name, if supplied. Otherwise defaults to the global workspace.
///
/// Also ensures the directory actually exists.
async fn get_create_workspace(state: &State, name: &str) -> Result<Workspace> {
    let name = if name.is_empty() { "global" } else { name };
    let ws = if let Some(ws) = state
        .get_workspace(name)
        .await
        .context("failed to retrieve workspace")?
    {
        ws
    } else {
        return Err(anyhow!("workspace {} does not exist", name));
    };

    Ok(ws)
}

/// Installs a package.
async fn install_package(state: &State, pkg: &str, workspace: &Workspace) -> Result<InstallLog> {
    let pkg_req: PackageRequest = pkg.parse().context("failed to parse package name")?;
    let pkg_spec: KnownPackageSpec = pkg_req
        .resolve_known_version(state)
        .await
        .context("failed to resolve package version")?;

    if state.get_workspace_package(pkg, workspace).await?.is_some() {
        return Err(anyhow!(
            "package {} is already installed in workspace {}",
            pkg,
            &workspace
        ));
    }

    let pkg = state.get_package(&pkg_spec).await?;
    let log = pkg.install(state, workspace).await?;

    if log.is_success() {
        if log.new_install {
            state.add_installed_package(&pkg_spec).await?;
        }
        state
            .add_workspace_package(&pkg_spec, workspace)
            .await
            .context("failed to register installed package")?;
    }

    Ok(log)
}

/// Updates a package.
async fn update_package(
    state: &State,
    pkg: &str,
    workspace: &Workspace,
) -> Result<Option<InstallLog>> {
    let pkg_req: PackageRequest = pkg.parse().context("failed to parse package name")?;
    let existing_pkg = pkg_req
        .resolve_workspace_version(state, workspace)
        .await
        .context("failed to resolve package version")?;

    if let Some(new_pkg) = existing_pkg.available_update(state).await? {
        // Install the new version
        let log = state
            .get_package(&new_pkg)
            .await?
            .install(state, workspace)
            .await?;
        // Remove the old one
        existing_pkg.remove(workspace).await?;
        state
            .remove_workspace_package(&existing_pkg, workspace)
            .await
            .context("failed to deregister installed package")?;
        Ok(Some(log))
    } else {
        Ok(None)
    }
}

/// Uninstalls a package.
async fn uninstall_package(state: &State, pkg: &str, workspace: &Workspace) -> Result<String> {
    let pkg_req: PackageRequest = pkg.parse().context("failed to parse package name")?;
    let pkg_spec: WorkspacePackageSpec = pkg_req
        .resolve_workspace_version(state, workspace)
        .await
        .context("failed to resolve package version")?;

    workspace
        .remove_package(&pkg_spec)
        .await
        .context("failed to remove package from workspace")?;
    state
        .remove_workspace_package(&pkg_spec, workspace)
        .await
        .context("failed to deregister installed package")?;

    Ok(format!("Uninstalled {pkg_spec}"))
}

/// Lists all installed packages.
async fn list_packages(state: &State, workspace: &Workspace) -> Result<()> {
    let packages = state.workspace_packages(workspace).await?;

    for pkg in packages {
        println!("{}", pkg);
    }

    Ok(())
}

/// Adds a registry.
async fn add_registry(state: &State, uri: &str, fetcher: &impl Fetcher) -> Result<()> {
    let mut registry = Registry::new(uri);
    registry.initialize(state, fetcher).await?;

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
            set.spawn(async move { registry.fetch(&state, &fetcher).await });
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
    if !is_file_system_safe(name) {
        return Err(anyhow!("workspace names can contain [a-zA-Z0-9._-] only"));
    }

    if state.get_workspace(name).await?.is_some() {
        return Err(anyhow!("workspace {} already exists", name));
    }

    state.add_workspace(&Workspace::new(name).await?).await?;
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
async fn workspace_shell(state: &State, workspace_name: &str) -> Result<()> {
    let Some(workspace) = state.get_workspace(workspace_name).await? else {
        return Err(anyhow!("workspace {} does not exist", workspace_name));
    };

    let patched_path = format!(
        "{}:{}",
        workspace.bin_directory()?.to_str().ok_or(anyhow!(
            "failed to convert workspace bin directory to string"
        ))?,
        current_path()
    );
    let system_shell = var("SHELL").unwrap_or_else(|_| "zsh".to_string());
    tokio::process::Command::new(system_shell)
        .env("MATCHA_WORKSPACE", &workspace.name)
        .env("PATH", &patched_path)
        .spawn()
        .context("failed to run workspace shell")?
        .wait()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    use crate::{registry::MockFetcher, workspace::test_workspace};

    /// Convenience function to setup the default test state.
    async fn setup_state_with_registry() -> Result<State> {
        let state = State::load(":memory:").await?;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&state, &MockFetcher::default()).await?;
        ensure_registries_are_current(&state, &MockFetcher::default(), false).await?;
        Ok(state)
    }

    #[tokio::test]
    async fn test_install_package() -> Result<()> {
        let package_root = TempDir::new()?;
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("global").await;

        let pkg: PackageRequest = "test-package@0.1.0".parse()?;
        let pkg: KnownPackageSpec = pkg.resolve_known_version(&state).await?;

        install_package(&state, &pkg.name, &workspace).await?;
        assert!(state
            .get_workspace_package(&pkg.name, &workspace)
            .await?
            .is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_install_package_refuses_if_package_is_already_installed() {
        let package_root = TempDir::new().unwrap();
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await.unwrap();
        let (_root, workspace) = test_workspace("global").await;

        let pkg = "test-package@0.1.0";

        install_package(&state, pkg, &workspace).await.unwrap();
        let result = install_package(&state, pkg, &workspace).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_uninstall_package() -> Result<()> {
        let package_root = TempDir::new()?;
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("global").await;

        let pkg: PackageRequest = "test-package@0.1.0".parse()?;
        let pkg: KnownPackageSpec = pkg.resolve_known_version(&state).await?;

        install_package(&state, &pkg.name, &workspace).await?;
        uninstall_package(&state, &pkg.name, &workspace).await?;
        assert!(state
            .get_workspace_package(&pkg.name, &workspace)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_uninstall_package_refuses_if_package_is_not_installed() {
        let state = setup_state_with_registry().await.unwrap();
        let (_root, workspace) = test_workspace("global").await;

        let pkg = "test-package@0.1.0";

        let result = uninstall_package(&state, pkg, &workspace).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_packages() {
        let package_root = TempDir::new().unwrap();
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await.unwrap();
        let (_root, workspace) = test_workspace("global").await;

        let pkg = "test-package@0.1.0";

        install_package(&state, pkg, &workspace).await.unwrap();
        list_packages(&state, &workspace).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_packages_empty() {
        let state = setup_state_with_registry().await.unwrap();
        let (_root, workspace) = test_workspace("global").await;
        list_packages(&state, &workspace).await.unwrap();
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
        remove_registry(&state, "https://example.invalid")
            .await
            .unwrap();
        assert!(!state
            .registry_exists("https://example.invalid")
            .await
            .unwrap());
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
        let (_root, _workspace) = test_workspace("global").await;

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
        let (_root, _workspace) = test_workspace("global").await;

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
        let (_root, _workspace) = test_workspace("global").await;

        let name = "test";

        add_workspace(&state, name).await.unwrap();
        list_workspaces(&state).await.unwrap();
    }

    #[tokio::test]
    async fn test_remove_workspace_with_packages() -> Result<()> {
        let package_root = TempDir::new()?;
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("test").await;

        add_workspace(&state, &workspace.name).await?;
        install_package(&state, "test-package@0.1.0", &workspace).await?;
        remove_workspace(&state, "test").await?;
        assert!(state
            .get_workspace_package("test-package@0.1.0", &workspace)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_add_workspace_rejects_invalid_names() {
        let state = State::load(":memory:").await.unwrap();
        let name = "test!";

        let result = add_workspace(&state, name).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_registry_picks_up_new_packages() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::with_packages(&[]))
            .await
            .unwrap();
        registry
            .fetch(&state, &MockFetcher::with_packages(&[]))
            .await
            .unwrap();
        assert!(state
            .known_packages_for_registry(&registry)
            .await
            .unwrap()
            .is_empty());

        ensure_registries_are_current(&state, &MockFetcher::default(), true)
            .await
            .unwrap();
        assert!(!state
            .known_packages_for_registry(&registry)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_update_registry_removes_gone_packages() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();
        registry
            .fetch(&state, &MockFetcher::default())
            .await
            .unwrap();
        assert!(!state
            .known_packages_for_registry(&registry)
            .await
            .unwrap()
            .is_empty());

        ensure_registries_are_current(&state, &MockFetcher::with_packages(&[]), true)
            .await
            .unwrap();
        assert!(state
            .known_packages_for_registry(&registry)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_get_create_workspace_defaults_to_global() {
        let state = State::load(":memory:").await.unwrap();
        let (_root, _workspace) = test_workspace("global").await;
        let workspace = get_create_workspace(&state, "").await.unwrap();
        assert_eq!(workspace.name, "global");
    }

    #[tokio::test]
    async fn test_get_create_workspace_refuses_nonexistent() {
        let state = State::load(":memory:").await.unwrap();
        let (_root, _workspace) = test_workspace("global").await;
        let result = get_create_workspace(&state, "test").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_install_package_doesnt_add_package_to_state_if_build_failed() -> Result<()> {
        let package_root = TempDir::new()?;
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("global").await;

        let pkg: PackageRequest = "failing-build@0.1.0".parse()?;
        let pkg: KnownPackageSpec = pkg.resolve_known_version(&state).await?;

        let result = install_package(&state, &pkg.name, &workspace).await;
        assert!(result.is_ok());
        assert!(state
            .get_workspace_package(&pkg.name, &workspace)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_install_uninstall_reinstall_package() -> Result<()> {
        let package_root = TempDir::new()?;
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("global").await;

        let pkg: PackageRequest = "test-package@0.1.0".parse()?;
        let pkg: KnownPackageSpec = pkg.resolve_known_version(&state).await?;
        install_package(&state, &pkg.name, &workspace).await?;
        uninstall_package(&state, &pkg.name, &workspace).await?;
        install_package(&state, &pkg.name, &workspace).await?;

        Ok(())
    }
}

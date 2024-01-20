//! User-facing command implementations.
//!
//! Anything public in this module is exposed as a command-line subcommand.

use std::env::var;

use anyhow::{anyhow, Context, Result};
use tokio::task::JoinSet;

use crate::{
    manifest::InstallLog,
    package::{KnownPackage, PackageChangeSet, PackageRequest, WorkspacePackage},
    registry::{Fetcher, Registry},
    state::State,
    util::is_file_system_safe,
    workspace::Workspace,
};

/// Installs a package.
pub async fn install_packages(state: &State, pkgs: &[String], workspace_name: &str) -> Result<()> {
    let pkg_reqs: Vec<PackageRequest> = pkgs
        .iter()
        .map(|pkg| pkg.parse::<PackageRequest>())
        .collect::<Result<Vec<_>>>()?;

    let workspace = get_create_workspace(state, workspace_name).await?;

    let workspace_packages = state.workspace_packages(&workspace).await?;
    let changeset = PackageChangeSet::add_packages(&pkg_reqs, &workspace_packages)?;

    let mut set = JoinSet::new();

    for pkg in changeset.added_packages() {
        let state = state.clone();
        let workspace = workspace.clone();
        set.spawn(async move { install_package(&state, &pkg, &workspace).await });
    }

    // TODO: Also apply changed packages.

    let mut results = vec![];
    while let Some(result) = set.join_next().await {
        results.push(result?);
    }
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

    Ok(())
}

/// Installs a package in the given workspace.
async fn install_package(
    state: &State,
    request: &PackageRequest,
    workspace: &Workspace,
) -> Result<InstallLog> {
    let pkg_spec: KnownPackage = request
        .resolve_known_version(state)
        .await
        .context("failed to resolve package version")?;

    let pkg = state
        .get_known_package(&pkg_spec)
        .await?
        .expect("package not found");
    let log = pkg.install(state, workspace).await?;

    if log.is_success() {
        if log.new_install {
            state.add_installed_package(&pkg_spec).await?;
        }
        let workspace_package = WorkspacePackage::from_request(request, &pkg.version);
        state
            .add_workspace_package(&workspace_package, workspace)
            .await
            .context("failed to register installed package")?;
    }

    Ok(log)
}

/// Updates the given packages.
pub async fn update_packages(state: &State, pkgs: &[String], workspace_name: &str) -> Result<()> {
    let workspace = get_create_workspace(state, workspace_name).await?;

    let pkgs = if pkgs.is_empty() {
        state
            .workspace_packages(&workspace)
            .await?
            .into_iter()
            .map(|pkg| pkg.name)
            .collect()
    } else {
        pkgs.to_vec()
    };

    let pkg_reqs: Vec<PackageRequest> = pkgs
        .into_iter()
        .map(|pkg| pkg.parse::<PackageRequest>())
        .collect::<Result<Vec<_>>>()?;

    let workspace_packages = state.workspace_packages(&workspace).await?;
    let changeset = PackageChangeSet::update_packages(&pkg_reqs, &workspace_packages)?;

    let mut set = JoinSet::new();

    for pkg in changeset.changed_packages() {
        let state = state.clone();
        let workspace = workspace.clone();
        set.spawn(async move { update_package(&state, &pkg, &workspace).await });
    }

    let mut results = vec![];
    while let Some(result) = set.join_next().await {
        results.push(result?);
    }
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

    Ok(())
}

/// Updates a package.
async fn update_package(
    state: &State,
    pkg: &PackageRequest,
    workspace: &Workspace,
) -> Result<Option<InstallLog>> {
    let existing_pkg = pkg
        .resolve_workspace_version(state, workspace)
        .await
        .context("failed to resolve package version")?;

    if let Some(new_pkg) = existing_pkg.available_update(state).await? {
        // Install the new version
        let log = state
            .get_known_package(&new_pkg)
            .await?
            .expect("package not found")
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

pub async fn remove_packages(state: &State, pkgs: &[String], workspace_name: &str) -> Result<()> {
    let workspace = get_create_workspace(state, workspace_name).await?;

    let pkg_reqs: Vec<PackageRequest> = pkgs
        .iter()
        .map(|pkg| pkg.parse::<PackageRequest>())
        .collect::<Result<Vec<_>>>()?;

    let workspace_packages = state.workspace_packages(&workspace).await?;
    let changeset = PackageChangeSet::remove_packages(&pkg_reqs, &workspace_packages)?;

    let mut set = JoinSet::new();

    for pkg in changeset.removed_packages() {
        let state = state.clone();
        let workspace = workspace.clone();
        set.spawn(async move { remove_package(&state, &pkg, &workspace).await });
    }

    let mut results = vec![];
    while let Some(result) = set.join_next().await {
        results.push(result?);
    }
    let output = results.into_iter().collect::<Result<Vec<String>>>()?;
    for line in output {
        println!("{}", line);
    }
    Ok(())
}

/// Removes a package from the given workspace.
pub async fn remove_package(
    state: &State,
    pkg: &PackageRequest,
    workspace: &Workspace,
) -> Result<String> {
    let pkg_spec: WorkspacePackage = pkg
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

/// Garbage collects all installed packages that are not referenced by any workspace.
pub async fn garbage_collect_installed_packages(state: &State) -> Result<()> {
    let packages = state.unused_installed_packages().await?;
    let count = packages.len() as u64;
    let mut set = JoinSet::new();

    for package in packages {
        let state = state.clone();
        set.spawn(async move {
            package
                .delete()
                .await
                .context("failed to delete unused package")?;
            state.remove_installed_package(&package).await?;
            Ok(())
        });
    }

    let mut results = vec![];
    while let Some(result) = set.join_next().await {
        results.push(result?);
    }

    results
        .into_iter()
        .collect::<Result<()>>()
        .context("failed to garbage collect packages")?;

    println!("Garbage collected {} packages", count);

    Ok(())
}

/// Lists all installed packages.
pub async fn list_packages(state: &State, workspace_name: &str) -> Result<()> {
    let workspace = get_create_workspace(state, workspace_name).await?;
    let packages = state.workspace_packages(&workspace).await?;

    for pkg in packages {
        println!("{}", pkg);
    }

    Ok(())
}

/// Adds a registry.
pub async fn add_registry(state: &State, uri: &str, fetcher: &impl Fetcher) -> Result<()> {
    let mut registry = Registry::new(uri);
    registry.initialize(state, fetcher).await?;

    eprintln!("Added registry {}", registry);
    Ok(())
}

/// Removes a registry.
pub async fn remove_registry(state: &State, uri: &str) -> Result<()> {
    state.remove_registry(uri).await?;

    eprintln!("Removed registry {}", uri);
    Ok(())
}

/// Lists all registries.
pub async fn list_registries(state: &State) -> Result<()> {
    let registries = state.registries().await?;

    for registry in registries {
        println!("{}", registry);
    }

    Ok(())
}

/// Ensures all registries are up to date by potentially refetching them.
///
/// Supply `force` to force a refetch of all registries.
pub async fn fetch_registries(
    state: &State,
    fetcher: &(impl Fetcher + 'static),
    force: bool,
) -> Result<()> {
    let registries = state.registries().await?;

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
        results.push(result?);
    }

    results
        .into_iter()
        .collect::<Result<()>>()
        .context("failed to update registries")?;

    Ok(())
}

/// Searches for a package.
pub async fn search_packages(state: &State, query: &str, all_versions: bool) -> Result<()> {
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

/// Shows information about a package.
pub async fn show_package(state: &State, pkg: &str) -> Result<()> {
    let pkg = pkg
        .parse::<PackageRequest>()
        .context("failed to parse package request")?;
    let pkg = pkg
        .resolve_known_version(state)
        .await
        .context("failed to resolve known package")?;
    let pkg = state
        .get_known_package(&pkg)
        .await?
        .ok_or_else(|| anyhow!("package not found"))?;
    println!("{:?}", pkg);
    Ok(())
}

/// Adds a workspace.
pub async fn add_workspace(state: &State, name: &str) -> Result<()> {
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
pub async fn remove_workspace(state: &State, name: &str) -> Result<()> {
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
pub async fn list_workspaces(state: &State) -> Result<()> {
    let workspaces = state.workspaces().await?;

    for workspace in workspaces {
        println!("{}", workspace);
    }

    Ok(())
}

/// Runs a shell in the context of a workspace.
pub async fn workspace_shell(state: &State, workspace_name: &str) -> Result<()> {
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

/// Returns the current value of $PATH.
fn current_path() -> String {
    var("PATH").unwrap_or_else(|_| "".to_string())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    use crate::{registry::MockFetcher, workspace::test_workspace};

    /// Convenience function to setup the default test state.
    ///
    /// Make sure to keep the package root in scope, otherwise it will be deleted.
    async fn setup_state_with_registry() -> Result<(State, TempDir)> {
        let package_root = TempDir::new().unwrap();
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();

        let state = State::load(":memory:").await?;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&state, &MockFetcher::default()).await?;
        fetch_registries(&state, &MockFetcher::default(), false).await?;
        Ok((state, package_root))
    }

    #[tokio::test]
    async fn test_remove_workspace_with_packages() -> Result<()> {
        let (state, _package_root) = setup_state_with_registry().await?;
        let (workspace, _workspace_root) = test_workspace("test").await;

        add_workspace(&state, &workspace.name).await?;
        install_package(&state, &"test-package@0.1.0".parse()?, &workspace).await?;
        remove_workspace(&state, &workspace.name).await?;
        assert!(state
            .get_workspace_package("test-package@0.1.0", &workspace)
            .await?
            .is_none());
        Ok(())
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

        fetch_registries(&state, &MockFetcher::default(), true)
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

        fetch_registries(&state, &MockFetcher::with_packages(&[]), true)
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
        let (state, _package_root) = setup_state_with_registry().await?;
        let (workspace, _workspace_root) = test_workspace("global").await;

        let pkg = "failing-build@0.1.0".parse()?;

        let result = install_package(&state, &pkg, &workspace).await;
        assert!(result.is_ok());
        assert!(state
            .get_workspace_package(&pkg.name, &workspace)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_garbage_collect_installed_packages() -> Result<()> {
        let (state, _package_root) = setup_state_with_registry().await?;
        let (workspace, _workspace_root) = test_workspace("global").await;

        let pkg = "test-package@0.1.0".parse()?;
        install_package(&state, &pkg, &workspace).await?;
        remove_package(&state, &pkg, &workspace).await?;

        assert!(state.get_installed_package(&pkg).await?.is_some());
        garbage_collect_installed_packages(&state).await?;
        assert!(state.get_installed_package(&pkg).await?.is_none());

        Ok(())
    }

    mod integration {
        use super::*;

        #[tokio::test]
        async fn test_install_different_version_in_workspace() -> Result<()> {
            let (state, _package_root) = setup_state_with_registry().await?;
            let (workspace, _workspace_root) = test_workspace("global").await;

            let pkgs = vec!["test-package@0.1.0".to_string()];
            install_packages(&state, &pkgs, &workspace.name).await?;

            add_workspace(&state, "test").await?;

            let pkgs = vec!["test-package@0.1.1".to_string()];
            install_packages(&state, &pkgs, "test").await?;

            Ok(())
        }
    }
}

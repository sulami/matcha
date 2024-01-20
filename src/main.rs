use std::{ops::Deref, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use once_cell::sync::OnceCell;
use shellexpand::tilde;

pub(crate) mod command;
pub(crate) mod download;
pub(crate) mod error;
pub(crate) mod manifest;
pub(crate) mod package;
pub(crate) mod registry;
pub(crate) mod state;
pub(crate) mod util;
pub(crate) mod workspace;

use crate::command::*;

use registry::DefaultFetcher;

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
                fetch_registries(&state, &DefaultFetcher, false).await?;
                install_packages(&state, &pkgs, &workspace).await?;
            }
            PackageCommand::Update { pkgs, workspace } => {
                fetch_registries(&state, &DefaultFetcher, false).await?;
                update_packages(&state, &pkgs, &workspace).await?;
            }
            PackageCommand::Remove { pkgs, workspace } => {
                remove_packages(&state, &pkgs, &workspace).await?
            }
            PackageCommand::Search {
                query,
                all_versions,
            } => {
                fetch_registries(&state, &DefaultFetcher, false).await?;
                search_packages(&state, &query, all_versions).await?;
            }
            PackageCommand::Show { pkg } => show_package(&state, &pkg).await?,
            PackageCommand::List { workspace } => list_packages(&state, &workspace).await?,
            PackageCommand::GarbageCollect => garbage_collect_installed_packages(&state).await?,
        },
        Command::Workspace(cmd) => match cmd {
            WorkspaceCommand::Add { workspace } => add_workspace(&state, &workspace).await?,
            WorkspaceCommand::Remove { workspace } => remove_workspace(&state, &workspace).await?,
            WorkspaceCommand::List => list_workspaces(&state).await?,
            WorkspaceCommand::Shell { workspace } => workspace_shell(&state, &workspace).await?,
        },
        Command::Registry(cmd) => match cmd {
            RegistryCommand::Add { uri } => add_registry(&state, &uri, &DefaultFetcher).await?,
            RegistryCommand::Remove { uri } => remove_registry(&state, &uri).await?,
            RegistryCommand::List => list_registries(&state).await?,
            RegistryCommand::Fetch => fetch_registries(&state, &DefaultFetcher, true).await?,
        },
    }

    Ok(())
}

/// All the command line arguments.
#[derive(Parser, Debug)]
#[command(author, version, about = "A peaceful package manager")]
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

    /// Show details for a package
    Show {
        /// Package to show
        #[arg(required = true)]
        pkg: String,
    },

    /// Garbage collect all installed packages that are not referenced by any workspace (alias: gc)
    #[command(alias = "gc")]
    GarbageCollect,
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

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        process::{Command as StdCommand, Output, Stdio},
    };

    use anyhow::Result;
    use assert_cmd::prelude::*;
    use tempfile::TempDir;
    use tokio::process::Command;

    /// Setup required to run a command.
    struct TestSetup {
        #[allow(dead_code)]
        config_dir: TempDir,
        state_db: String,
        package_root: TempDir,
        workspace_root: TempDir,
    }

    impl Default for TestSetup {
        fn default() -> Self {
            let config_dir = TempDir::new().unwrap();
            let state_db = config_dir
                .as_ref()
                .to_owned()
                .join("state.db")
                .to_str()
                .unwrap()
                .to_string();
            Self {
                config_dir,
                state_db,
                package_root: TempDir::new().unwrap(),
                workspace_root: TempDir::new().unwrap(),
            }
        }
    }

    fn local_test_registry() -> String {
        PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
            .join("registry.toml")
            .to_str()
            .unwrap()
            .to_string()
    }

    /// Runs a command with the provided test setup, returning the result.
    async fn run_test_command(setup: &TestSetup, args: &[&str]) -> Result<Output> {
        let mut cmd: Command = StdCommand::cargo_bin("matcha")?.into();
        cmd.args(args)
            .env("MATCHA_STATE_DB", &setup.state_db)
            .env("MATCHA_PACKAGE_ROOT", setup.package_root.path())
            .env("MATCHA_WORKSPACE_ROOT", setup.workspace_root.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = cmd.spawn()?.wait_with_output().await?;
        Ok(output)
    }

    #[tokio::test]
    async fn test_install_a_package() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
        assert!(out.status.success());

        Ok(())
    }

    #[tokio::test]
    async fn test_install_two_packages() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(
            &setup,
            &["package", "install", "test-package", "another-package"],
        )
        .await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert_eq!(
            stdout,
            "test-package@0.1.1 (resolved from *)\nanother-package@0.2.0 (resolved from *)\n"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_install_two_different_versions() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package@0.1.1"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("conflicting requests for dependency 'test-package'"));

        Ok(())
    }

    #[tokio::test]
    async fn test_install_laxer_version() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package@~0.1"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
        assert!(out.status.success());

        Ok(())
    }

    #[tokio::test]
    async fn test_list_installed_packages() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert_eq!(stdout, "test-package@0.1.1 (resolved from *)\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_list_installed_packages_empty() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["package", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert!(stdout.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_show_package() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["registry", "fetch"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "show", "test-package"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert_eq!(stdout, "test-package@0.1.1\n  Registry: TODO");

        Ok(())
    }

    #[tokio::test]
    async fn test_show_unknown_package() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["package", "show", "test-package"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("package test-package is not known"));

        Ok(())
    }

    #[tokio::test]
    async fn test_unninstall_package() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "remove", "test-package"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["package", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert!(stdout.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_uninstall_unknown_package() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["package", "remove", "test-package"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("package test-package is not installed"));

        Ok(())
    }

    #[tokio::test]
    async fn test_list_registries() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["registry", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert_eq!(stdout, format!("{} (test)\n", &local_test_registry()));

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_add_duplicate_registry() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert_eq!(
            stderr,
            format!("registry {} already exists\n", &local_test_registry())
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_remove_registry() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["registry", "remove", &local_test_registry()]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["registry", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert!(stdout.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_remove_unknown_registry() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["registry", "remove", "unkonwn"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("registry unkonwn does not exist"));

        Ok(())
    }

    #[tokio::test]
    async fn test_add_workspace() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["workspace", "list"]).await?;
        assert!(out.status.success());

        let stdout = String::from_utf8(out.stdout)?;
        assert_eq!(stdout, "global\ntest-workspace\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_add_duplicate_workspace() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
        assert!(out.status.success());

        let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("workspace test-workspace already exists"));

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_remove_global_workspace() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["workspace", "remove", "global"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("cannot remove global workspace"));

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_remove_unknown_workspace() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["workspace", "remove", "unknown"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("workspace unknown does not exist"));

        Ok(())
    }

    #[tokio::test]
    async fn test_cannot_use_invalid_workspace_name() -> Result<()> {
        let setup = TestSetup::default();

        let out = run_test_command(&setup, &["workspace", "add", "test/workspace"]).await?;
        assert!(!out.status.success());

        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains("workspace names can contain"));

        Ok(())
    }
}

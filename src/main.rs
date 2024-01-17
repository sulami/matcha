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

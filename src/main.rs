use anyhow::{Context, Result};
use clap::Parser;

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
            for pkg in pkgs {
                let pkg = pkg.parse().context("failed to parse package name")?;
                // TODO: Deal with rollbacks for failed installs.
                state
                    .add_installed_package(&pkg)
                    .await
                    .context("failed to register installed package")?;
                println!("Installed {pkg}");
            }
        }
        Command::Uninstall { pkgs } => {
            for pkg in pkgs {
                uninstall_package(&state, &pkg).await?
            }
        }
        Command::List => {
            for pkg in state
                .list_installed_packages()
                .await
                .context("Failed to list installed packages")?
            {
                println!("{pkg}");
            }
        }
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
}

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

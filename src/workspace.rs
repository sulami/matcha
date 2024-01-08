use std::{fmt::Display, ops::Deref, path::PathBuf};

use anyhow::{Context, Result};
use sqlx::FromRow;

use crate::{package::InstalledPackageSpec, WORKSPACE_DIRECTORY};

/// A place that can have packages installed.
#[derive(Debug, Clone, FromRow)]
pub struct Workspace {
    /// The name of the workplace.
    pub name: String,
}

impl Workspace {
    /// Creates a new workspace.
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
        }
    }

    /// Returns the directory of the workspace.
    pub fn directory(&self) -> Result<PathBuf> {
        let workspace_directory = WORKSPACE_DIRECTORY
            .get()
            .context("workspace directory not initialized")?;
        let dir = shellexpand::tilde(workspace_directory.join(&self.name).to_str().unwrap())
            .deref()
            .into();
        Ok(dir)
    }

    /// Returns the bin directory of the workspace.
    pub fn bin_directory(&self) -> Result<PathBuf> {
        Ok(self.directory()?.join("bin"))
    }

    /// Creates the directory for the workspace, if it doesn't exist.
    pub async fn ensure_exists(&self) -> Result<()> {
        tokio::fs::create_dir_all(self.directory()?.join("bin"))
            .await
            .context("failed to create workspace root")?;
        Ok(())
    }

    /// Removes a package's files from this workspace.
    pub async fn remove_package(&self, pkg: &InstalledPackageSpec) -> Result<()> {
        // TODO: Remove the package's files.
        Ok(())
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new("global")
    }
}

impl Display for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

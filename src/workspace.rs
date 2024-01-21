use std::{fmt::Display, ops::Deref, path::PathBuf};

use color_eyre::eyre::{eyre, Result, WrapErr};
use shellexpand::tilde;
use sqlx::FromRow;
use tokio::fs::{create_dir_all, read_dir, read_link, remove_file};
use tracing::instrument;

use crate::{
    package::{InstalledPackage, WorkspacePackage},
    WORKSPACE_ROOT,
};

/// A place that can have packages installed.
#[derive(Debug, Clone, FromRow)]
pub struct Workspace {
    /// The name of the workplace.
    pub name: String,
}

impl Workspace {
    /// Creates a new workspace.
    ///
    /// Also ensures the workspace(/bin) directory exists.
    #[instrument]
    pub async fn new(name: &str) -> Result<Self> {
        let ws = Self {
            name: String::from(name),
        };
        ws.ensure_exists().await?;
        Ok(ws)
    }

    /// Returns the directory of the workspace.
    #[instrument]
    pub fn directory(&self) -> Result<PathBuf> {
        let workspace_directory = WORKSPACE_ROOT
            .get()
            .ok_or_else(|| eyre!("workspace directory not initialized"))?;
        let dir = tilde(workspace_directory.join(&self.name).to_str().unwrap())
            .deref()
            .into();
        Ok(dir)
    }

    /// Returns the bin directory of the workspace.
    #[instrument]
    pub fn bin_directory(&self) -> Result<PathBuf> {
        Ok(self
            .directory()
            .wrap_err("failed to get workspace bin directory")?
            .join("bin"))
    }

    /// Creates the directory for the workspace, if it doesn't exist.
    #[instrument]
    async fn ensure_exists(&self) -> Result<()> {
        create_dir_all(self.directory()?.join("bin"))
            .await
            .wrap_err("failed to create workspace root")?;
        Ok(())
    }

    /// Removes a package's files from this workspace.
    #[instrument]
    pub async fn remove_package(&self, pkg: &WorkspacePackage) -> Result<()> {
        let installed_package = InstalledPackage::from(pkg);
        let pkg_dir = installed_package.directory();

        // Remove the package's bin symlinks.
        let mut bin_dir_reader = read_dir(self.bin_directory()?).await?;
        while let Some(entry) = bin_dir_reader.next_entry().await? {
            if entry.metadata().await?.file_type().is_symlink()
                && read_link(entry.path()).await?.starts_with(&pkg_dir)
            {
                remove_file(entry.path())
                    .await
                    .wrap_err("failed to delete package bin symlink")?;
            }
        }

        Ok(())
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            name: String::from("default"),
        }
    }
}

impl Display for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

#[cfg(test)]
/// Creates a test workspace, and also sets the workspace_root to a temporary directory.
pub async fn test_workspace(name: &str) -> (Workspace, tempfile::TempDir) {
    let workspace_root = tempfile::tempdir().expect("failed to create test workspace root");
    crate::WORKSPACE_ROOT
        .set(workspace_root.path().to_owned())
        .expect("failed to set workspace root");
    (
        Workspace::new(name)
            .await
            .expect("failed to create test workspace"),
        workspace_root,
    )
}

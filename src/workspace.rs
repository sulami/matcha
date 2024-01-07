use std::fmt::Display;

use anyhow::{Context, Result};
use sqlx::FromRow;

use crate::WORKSPACE_DIRECTORY;

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

    /// Creates the directory for the workspace, if it doesn't exist.
    pub async fn ensure_exists(&self) -> Result<()> {
        let workspace_directory = WORKSPACE_DIRECTORY
            .get()
            .context("workspace directory not initialized")?;
        tokio::fs::create_dir_all(workspace_directory.join(&self.name))
            .await
            .context("failed to create workspace root")?;
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

use std::fmt::Display;

use sqlx::FromRow;

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

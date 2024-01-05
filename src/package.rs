use std::{fmt::Display, str::FromStr};

use anyhow::{anyhow, Error};

/// A package.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Package {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    ///
    /// This can be `None` if the package has not been parsed with a version.
    pub version: Option<String>,
}

impl Package {
    pub fn is_fully_qualified(&self) -> bool {
        if let Some(version) = &self.version {
            version != "latest"
        } else {
            false
        }
    }
}

impl Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(version) = &self.version {
            write!(f, "@{}", version)?;
        }
        Ok(())
    }
}

impl FromStr for Package {
    type Err = Error;

    /// Parses a package name and version from a string.
    ///
    /// The format is <package>[@<version>], where version defaults to "latest".
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '@');
        let name = parts
            .next()
            .ok_or(anyhow!("failed to parse package name"))?;
        let version = parts.next();
        Ok(Self {
            name: name.to_string(),
            version: version.map(|v| v.to_string()),
        })
    }
}

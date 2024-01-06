use std::{fmt::Display, str::FromStr};

use anyhow::{anyhow, Error, Result};

use crate::state::State;

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
    /// Resolves this package to its latest installed version.
    ///
    /// Returns an error if the package is either not installed,
    /// or if multiple versions of the package are installed.
    pub async fn resolve_version(&mut self, state: &State) -> Result<()> {
        if !self.is_fully_qualified() {
            let installed_versions = state.installed_package_versions(self).await?;
            if installed_versions.is_empty() {
                return Err(anyhow!("package {} is not installed", self));
            }
            if installed_versions.len() > 1 {
                return Err(anyhow!(
                    "multiple versions of package {} are installed: {}",
                    self.name,
                    installed_versions.join(", ")
                ));
            }
            self.version = Some(installed_versions.first().unwrap().clone());
        } else if !state.is_package_installed(self).await? {
            return Err(anyhow!("package {} is not installed", self));
        }
        Ok(())
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

impl From<crate::manifest::Package> for Package {
    fn from(pkg: crate::manifest::Package) -> Self {
        Self {
            name: pkg.name,
            version: Some(pkg.version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package() {
        let pkg: Package = "foo".parse().unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, None);

        let pkg: Package = "foo@latest".parse().unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, Some("latest".to_string()));

        let pkg: Package = "foo@1.2.3".parse().unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, Some("1.2.3".to_string()));
    }

    #[test]
    fn test_is_fully_qualified() {
        let pkg: Package = "foo".parse().unwrap();
        assert!(!pkg.is_fully_qualified());

        let pkg: Package = "foo@latest".parse().unwrap();
        assert!(!pkg.is_fully_qualified());

        let pkg: Package = "foo@1.2.3".parse().unwrap();
        assert!(pkg.is_fully_qualified());
    }

    #[test]
    fn test_display() {
        let pkg: Package = "foo".parse().unwrap();
        assert_eq!(pkg.to_string(), "foo");

        let pkg: Package = "foo@latest".parse().unwrap();
        assert_eq!(pkg.to_string(), "foo@latest");

        let pkg: Package = "foo@1.2.3".parse().unwrap();
        assert_eq!(pkg.to_string(), "foo@1.2.3");
    }

    #[test]
    fn test_from_manifest_package() {
        let pkg: Package = crate::manifest::Package {
            name: "foo".to_string(),
            version: "1.2.3".to_string(),
            description: None,
            homepage: None,
            license: None,
            registry: "test".to_string(),
        }
        .into();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, Some("1.2.3".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_version() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: None,
        };
        pkg.resolve_version(&state).await.unwrap();
        assert_eq!(pkg.version, Some("1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_version_fails_if_not_installed() {
        let state = State::load(":memory:").await.unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: None,
        };
        assert!(pkg.resolve_version(&state).await.is_err());
    }

    #[tokio::test]
    async fn test_resolve_version_fails_if_this_version_is_not_installed() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: Some("2.0.0".to_string()),
        };
        assert!(pkg.resolve_version(&state).await.is_err());
    }

    #[tokio::test]
    async fn test_resolve_installed_package_version_fails_if_multiple_installed() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("2.0.0".to_string()),
            })
            .await
            .unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: None,
        };
        assert!(pkg.resolve_version(&state).await.is_err());
    }
}

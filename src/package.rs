use std::{fmt::Display, path::PathBuf, str::FromStr};

use anyhow::{anyhow, Context, Error, Result};
use sqlx::FromRow;
use tokio::fs::remove_dir_all;

use crate::{state::State, workspace::Workspace, PACKAGE_ROOT};

/// A package name and maybe a version, which needs resolution in some context.
#[derive(Clone, Debug)]
pub struct PackageRequest {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: Option<String>,
}

impl PackageRequest {
    /// If the version isn't fully qualified, resolves it to the latest installed one.
    ///
    /// Returns an error if the package is not installed in this workspace.
    pub async fn resolve_workspace_version(
        &self,
        state: &State,
        workspace: &Workspace,
    ) -> Result<WorkspacePackageSpec> {
        let Some(installed) = state.get_workspace_package(&self.name, workspace).await? else {
            return Err(anyhow!("package {} is not installed", self));
        };

        let Some(resolved) = find_matching_version(
            &[installed.version.clone()],
            self.version.as_deref().unwrap_or_default(),
        ) else {
            return Err(anyhow!(
                "package {} is not installed, but this versions is: {}",
                self,
                installed.version
            ));
        };

        Ok(WorkspacePackageSpec::from_request(self, resolved))
    }

    /// If the version isn't fully qualified, resolves it to the latest known one.
    ///
    /// Returns an error if the package is not known.
    /// If multiple versions of the package are known, the first (latest) one that matches is used.
    pub async fn resolve_known_version(&self, state: &State) -> Result<KnownPackageSpec> {
        let known_versions = state.known_package_versions(&self.name).await?;

        if known_versions.is_empty() {
            return Err(anyhow!("package {} is not known", self));
        }

        let Some(resolved) =
            find_matching_version(&known_versions, self.version.as_deref().unwrap_or_default())
        else {
            return Err(anyhow!(
                "package {} is not known, but these versions are: {}",
                self,
                known_versions.join(", ")
            ));
        };

        Ok(KnownPackageSpec::from_request(self, resolved))
    }
}

impl Display for PackageRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(version) = &self.version {
            write!(f, "@{}", version)?;
        }
        Ok(())
    }
}

impl FromStr for PackageRequest {
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

impl From<crate::manifest::Package> for PackageRequest {
    fn from(pkg: crate::manifest::Package) -> Self {
        Self {
            name: pkg.name,
            version: Some(pkg.version),
        }
    }
}

/// Returns the first version in `haystack` that starts with `needle`.
///
/// Considers that for e.g. semantic versioning, "1" does not match "10.0.0".
/// If `needle` is empty, returns the first version in `haystack`.
fn find_matching_version(haystack: &[String], needle: &str) -> Option<String> {
    if needle.is_empty() {
        return haystack.first().cloned();
    }
    haystack
        .iter()
        .find(|v| {
            v.starts_with(needle)
                && (v.len() == needle.len() || !v.as_bytes()[needle.len()].is_ascii_digit())
        })
        .cloned()
}

/// A [`PackageRequest`] with a resolved version based on known packages.
#[derive(Clone, Debug, FromRow)]
pub struct KnownPackageSpec {
    /// The name of the package.
    pub name: String,
    /// The resolved version of the package.
    pub version: String,
    /// The unresolved version that was requested.
    pub requested_version: String,
}

impl KnownPackageSpec {
    /// Create a new spec from a [`PackageRequest`] and a resolved version.
    pub fn from_request(request: &PackageRequest, version: String) -> Self {
        Self {
            name: request.name.clone(),
            version,
            requested_version: request.version.clone().unwrap_or_default(),
        }
    }
}

impl Display for KnownPackageSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// Test-only crutch.
#[cfg(test)]
impl From<WorkspacePackageSpec> for KnownPackageSpec {
    fn from(spec: WorkspacePackageSpec) -> Self {
        Self {
            name: spec.name,
            version: spec.version,
            requested_version: spec.requested_version,
        }
    }
}

/// A [`PackageRequest`] with a resolved version based packages in a workspace.
#[derive(Clone, Debug, FromRow)]
pub struct WorkspacePackageSpec {
    /// The name of the package.
    pub name: String,
    /// The resolved version of the package.
    pub version: String,
    /// The unresolved version that was requested.
    pub requested_version: String,
}

impl WorkspacePackageSpec {
    /// Creates a new spec from a [`PackageRequest`] and a resolved version.
    pub fn from_request(request: &PackageRequest, version: String) -> Self {
        Self {
            name: request.name.clone(),
            version,
            requested_version: request.version.clone().unwrap_or_default(),
        }
    }

    /// Returns the directory of this package.
    pub fn directory(&self) -> PathBuf {
        PACKAGE_ROOT
            .get()
            .expect("uninitialized package root")
            .join(&self.name)
            .join(&self.version)
    }

    /// Deletes this package's files from the package root.
    pub async fn delete(&self) -> Result<()> {
        let dir = self.directory();
        remove_dir_all(dir).await?;
        Ok(())
    }

    /// Returns the latest known version of this package, if it is newer than the installed one.
    pub async fn available_update(&self, state: &State) -> Result<Option<KnownPackageSpec>> {
        let known_versions = state.known_package_versions(&self.name).await?;
        let Some(latest) = find_matching_version(&known_versions, &self.requested_version) else {
            return Ok(None);
        };
        if self.version < latest {
            Ok(Some(KnownPackageSpec {
                name: self.name.clone(),
                version: latest,
                requested_version: self.requested_version.clone(),
            }))
        } else {
            Ok(None)
        }
    }

    /// Removes this package's files from a workspace.
    pub async fn remove(&self, workspace: &Workspace) -> Result<()> {
        workspace
            .remove_package(self)
            .await
            .context("failed to remove package from workspace")
    }
}

impl Display for WorkspacePackageSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}@{} from {}",
            self.name,
            self.version,
            if self.requested_version.is_empty() {
                "latest"
            } else {
                &self.requested_version
            }
        )
    }
}

// Test-only crutch.
#[cfg(test)]
impl From<KnownPackageSpec> for WorkspacePackageSpec {
    fn from(spec: KnownPackageSpec) -> Self {
        Self {
            name: spec.name,
            version: spec.version,
            requested_version: spec.requested_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        manifest::Package as ManifestPackage,
        registry::{MockFetcher, Registry},
        workspace::test_workspace,
    };

    /// Returns a known package spec with the given name and version.
    fn known_package(name: &str, version: &str) -> KnownPackageSpec {
        KnownPackageSpec {
            name: name.to_string(),
            version: version.to_string(),
            requested_version: version.to_string(),
        }
    }

    #[test]
    fn test_parse_package() {
        let pkg: PackageRequest = "foo".parse().unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, None);

        let pkg: PackageRequest = "foo@latest".parse().unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, Some("latest".to_string()));

        let pkg: PackageRequest = "foo@1.2.3".parse().unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, Some("1.2.3".to_string()));
    }

    #[test]
    fn test_display() {
        let pkg: PackageRequest = "foo".parse().unwrap();
        assert_eq!(pkg.to_string(), "foo");

        let pkg: PackageRequest = "foo@latest".parse().unwrap();
        assert_eq!(pkg.to_string(), "foo@latest");

        let pkg: PackageRequest = "foo@1.2.3".parse().unwrap();
        assert_eq!(pkg.to_string(), "foo@1.2.3");
    }

    #[test]
    fn test_from_manifest_package() {
        let pkg: PackageRequest = crate::manifest::Package {
            name: "foo".to_string(),
            version: "1.2.3".to_string(),
            registry: "test".to_string(),
            ..Default::default()
        }
        .into();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, Some("1.2.3".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_version() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("foo", "1.0.0");
        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let pkg = PackageRequest {
            name: "foo".to_string(),
            version: None,
        };
        let spec = pkg.resolve_workspace_version(&state, &workspace).await?;
        assert_eq!(spec.version, "1.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_version_fails_if_not_installed() {
        let state = State::load(":memory:").await.unwrap();
        let (_root, workspace) = test_workspace("global").await;
        let pkg = PackageRequest {
            name: "foo".to_string(),
            version: None,
        };
        assert!(pkg
            .resolve_workspace_version(&state, &workspace)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_resolve_version_fails_if_this_version_is_not_installed() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("foo", "1.0.0");
        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let pkg = PackageRequest {
            name: "foo".to_string(),
            version: Some("2".to_string()),
        };
        assert!(pkg
            .resolve_workspace_version(&state, &workspace)
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_known_version() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();
        state
            .add_known_packages(&[ManifestPackage {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                registry: "https://example.invalid/registry".to_string(),
                ..Default::default()
            }])
            .await
            .unwrap();
        let pkg = PackageRequest {
            name: "foo".to_string(),
            version: None,
        };
        let spec = pkg.resolve_known_version(&state).await.unwrap();
        assert_eq!(spec.version, "1.0.0");
    }

    #[tokio::test]
    async fn test_resolve_known_version_fails_if_not_known() {
        let state = State::load(":memory:").await.unwrap();
        let pkg = PackageRequest {
            name: "foo".to_string(),
            version: None,
        };
        assert!(pkg.resolve_known_version(&state).await.is_err());
    }

    #[tokio::test]
    async fn test_resolve_known_version_fails_if_this_version_is_not_known() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("foo", "1.0.0");

        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let pkg = PackageRequest {
            name: "foo".to_string(),
            version: Some("2.0.0".to_string()),
        };
        assert!(pkg.resolve_known_version(&state).await.is_err());
        Ok(())
    }

    #[test]
    fn test_find_matching_version() {
        // Varying levels of precision matches.
        assert_eq!(
            find_matching_version(&["1.0.0".to_string()], "1"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            find_matching_version(&["1.0.0".to_string()], "1.0"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            find_matching_version(&["1.0.0".to_string()], "1.0.0"),
            Some("1.0.0".to_string())
        );
        // No match.
        assert_eq!(find_matching_version(&["1.0.1".to_string()], "1.0.0"), None);
        // Not matching newer versions with same prefix.
        assert_eq!(
            find_matching_version(&["1.0.0".to_string(), "10.0.0".to_string()], "1"),
            Some("1.0.0".to_string())
        );
        // Empty needle matches the first version.
        assert_eq!(
            find_matching_version(&["1.0.0".to_string(), "10.0.0".to_string()], ""),
            Some("1.0.0".to_string())
        );
    }
}

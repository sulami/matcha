use std::{fmt::Display, path::PathBuf};

use anyhow::{Context, Result};
use sqlx::FromRow;
use tokio::fs::remove_dir_all;

use crate::{
    dependencies::DependencyRequest, manifest::Package, state::State, workspace::Workspace,
    PACKAGE_ROOT,
};

/// A package specification that includes a name and a version.
pub trait PackageSpec {
    /// Return the name and version of this package.
    ///
    /// The version is a known good one, such as a resolved or installed one.
    fn spec(&self) -> (String, String);
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
    /// Creates a new spec from a [`crate::manifest::Package`].
    pub fn from_manifest_package(pkg: &Package) -> Self {
        Self {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            requested_version: pkg.version.clone(),
        }
    }

    pub fn from_request(request: &DependencyRequest, version: &str) -> Self {
        Self {
            name: request.name.clone(),
            version: version.to_string(),
            requested_version: format!("{}", request.version),
        }
    }
}

impl PackageSpec for KnownPackageSpec {
    fn spec(&self) -> (String, String) {
        (self.name.clone(), self.version.clone())
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
    pub fn from_request(request: &DependencyRequest, version: &str) -> Self {
        Self {
            name: request.name.clone(),
            version: version.to_string(),
            requested_version: format!("{}", request.version),
        }
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

impl PackageSpec for WorkspacePackageSpec {
    fn spec(&self) -> (String, String) {
        (self.name.clone(), self.version.clone())
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

/// An installed package.
///
/// This is mostly a shorter alias for [`crate::manifest::Package`], which only has the name and
/// version, as it is stored in the database.
#[derive(Clone, Debug, FromRow)]
pub struct InstalledPackage {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: String,
}

impl InstalledPackage {
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
}

impl PackageSpec for InstalledPackage {
    fn spec(&self) -> (String, String) {
        (self.name.clone(), self.version.clone())
    }
}

impl From<&WorkspacePackageSpec> for InstalledPackage {
    fn from(spec: &WorkspacePackageSpec) -> Self {
        Self {
            name: spec.name.clone(),
            version: spec.version.clone(),
        }
    }
}

impl From<Package> for InstalledPackage {
    fn from(pkg: Package) -> Self {
        Self {
            name: pkg.name,
            version: pkg.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

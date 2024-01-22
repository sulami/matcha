use std::{fmt::Display, ops::BitAnd, path::PathBuf, str::FromStr};

use color_eyre::eyre::{anyhow, Context, Result};
use sqlx::FromRow;
use tokio::fs::remove_dir_all;
use tracing::instrument;

use crate::{
    error::{Conflicts, InvalidVersonSpec},
    manifest::Package,
    state::State,
    workspace::Workspace,
    PACKAGE_ROOT,
};

/// A package specification that includes a name and a version.
pub trait PackageSpec: std::fmt::Debug {
    /// Return the name and version of this package.
    ///
    /// The version is a known good one, such as a resolved or installed one.
    fn spec(&self) -> (String, String);
}

/// A set of changes to workspace packages.
#[derive(Debug, Default)]
pub struct PackageChangeSet {
    /// Packages that need to be added.
    add: Vec<PackageRequest>,
    /// Packages that need to be upgraded or downgraded.
    change: Vec<PackageRequest>,
    /// Packages that need to be removed.
    remove: Vec<PackageRequest>,
}

impl PackageChangeSet {
    /// Creates a changeset that adds the given packages.
    #[instrument]
    pub fn add_packages(
        pkgs: &[PackageRequest],
        workspace_packages: &[WorkspacePackage],
    ) -> Result<Self> {
        let mut changeset = Self {
            add: Vec::from(pkgs),
            ..Self::default()
        };

        changeset.resolve(workspace_packages)?;

        Ok(changeset)
    }

    /// Creates a changeset that updates the given packages.
    #[instrument]
    pub fn update_packages(
        pkgs: &[PackageRequest],
        workspace_packages: &[WorkspacePackage],
    ) -> Result<Self> {
        let mut change_set = Self {
            change: Vec::from(pkgs),
            ..Self::default()
        };

        change_set.resolve(workspace_packages)?;

        Ok(change_set)
    }

    /// Creates a changeset that removes the given packages.
    #[instrument]
    pub fn remove_packages(
        pkgs: &[PackageRequest],
        workspace_packages: &[WorkspacePackage],
    ) -> Result<Self> {
        let mut change_set = Self {
            remove: Vec::from(pkgs),
            ..Self::default()
        };

        change_set.resolve(workspace_packages)?;

        Ok(change_set)
    }

    /// Returns the packages that need to be added.
    pub fn added_packages(&self) -> impl Iterator<Item = PackageRequest> + '_ {
        self.add.iter().cloned()
    }

    /// Returns the packages that need to be upgraded or downgraded.
    pub fn changed_packages(&self) -> impl Iterator<Item = PackageRequest> + '_ {
        self.change.iter().cloned()
    }

    /// Returns the packages that need to be removed.
    pub fn removed_packages(&self) -> impl Iterator<Item = PackageRequest> + '_ {
        self.remove.iter().cloned()
    }

    /// Resolves the changeset based on the current workflow packages.
    #[instrument]
    fn resolve(&mut self, current: &[WorkspacePackage]) -> Result<()> {
        // Get all the requests currently in the workspace.
        let current_requests = current
            .iter()
            .map(|p| PackageRequest {
                name: p.name.clone(),
                version: p.requested_version.clone(),
            })
            .collect::<Vec<PackageRequest>>();

        // Merge the current requests with the new requests, removing the new requests from
        // self.add because they might not actually be needed. The ones that are needed are
        // re-added below.
        let merged_requests =
            merge_dependency_requests(current_requests.into_iter().chain(self.add.drain(..)))?;

        // TODO: This does not handle removals yet.

        for request in merged_requests {
            if let Some(existing) = current.iter().find(|p| p.name == request.name) {
                // The request matches a currently included package.
                if request.version.matches(&existing.version) {
                    // The currently included package satisfies the request, do nothing.
                    continue;
                }
                // The currently included package does not satisfy the request, add it to the
                // grade list to be upgraded or downgraded.
                self.change.push(request);
            } else {
                // The request does not match a currently included package, add it to the add list
                // to be installed.
                if !self.add.iter().any(|r| r.name == request.name) {
                    self.add.push(request);
                }
            }
        }

        Ok(())
    }
}

/// A request for a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageRequest {
    /// The name of the package.
    pub name: String,
    /// The requested version of the package.
    pub version: VersionSpec,
}

impl PackageRequest {
    /// Resolves this request to a known package that can be installed.
    ///
    /// If the version isn't fully qualified, resolves it to the latest known one. Returns an error
    /// if the package is not known. If multiple versions of the package are known, the first
    /// (latest) one that matches is used.
    #[instrument(skip(state))]
    pub async fn resolve_known_version(&self, state: &State) -> Result<KnownPackage> {
        let known_versions = state.known_package_versions(&self.name).await?;

        if known_versions.is_empty() {
            return Err(anyhow!("package {} is not known", self.name));
        }

        let Some(resolved) = known_versions.iter().find(|v| self.version.matches(v)) else {
            return Err(anyhow!(
                "package {} is not known, but these versions are: {}",
                self,
                known_versions.join(", ")
            ));
        };

        Ok(KnownPackage::from_request(self, resolved))
    }

    /// Resolves this request to a workspace package from the given workspace.
    ///
    /// If the version isn't fully qualified, resolves it to the latest installed one. Returns an
    /// error if the package is not installed in this workspace.
    #[instrument(skip(state))]
    pub async fn resolve_workspace_version(
        &self,
        state: &State,
        workspace: &Workspace,
    ) -> Result<WorkspacePackage> {
        let Some(installed) = state.get_workspace_package(&self.name, workspace).await? else {
            return Err(anyhow!("package {} is not installed", self));
        };

        if !self.version.matches(&installed.version) {
            return Err(anyhow!(
                "package {} is not in this workspace, but this versions is: {}",
                self,
                installed.version
            ));
        }

        Ok(WorkspacePackage::from_request(self, &installed.version))
    }
}

impl FromStr for PackageRequest {
    type Err = color_eyre::eyre::Error;

    #[instrument]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '@');
        let Some(name) = parts.next() else {
            color_eyre::eyre::bail!("invalid dependency request: {}", s);
        };
        let version = parts.next().unwrap_or("");
        Ok(Self {
            name: name.into(),
            version: version.parse()?,
        })
    }
}

impl From<WorkspacePackage> for PackageRequest {
    fn from(value: WorkspacePackage) -> Self {
        Self {
            name: value.name,
            version: value.version.parse().expect("invalid version spec"),
        }
    }
}

impl Display for PackageRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let VersionSpec::Any = self.version {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}@{}", self.name, self.version)
        }
    }
}

impl PackageSpec for PackageRequest {
    fn spec(&self) -> (String, String) {
        (self.name.clone(), self.version.to_string())
    }
}

/// A version spec, which can be used to resolve to a concrete version.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub enum VersionSpec {
    /// Any version at all.
    #[default]
    Any,
    /// A version matching this prefix.
    Partial(String),
    /// Exactly this version.
    Exact(String),
}

impl VersionSpec {
    /// Constructs a version spec that matches specifically `version`.
    pub fn exact(version: &str) -> Self {
        VersionSpec::Exact(version.into())
    }

    /// Constructs a version spec that matches any version with the given prefix.
    pub fn partial(prefix: &str) -> Self {
        VersionSpec::Partial(prefix.into())
    }

    /// Returns `true` if `version` matches this version spec.
    #[instrument]
    fn matches(&self, version: &str) -> bool {
        match self {
            VersionSpec::Any => true,
            VersionSpec::Exact(exact) => version == exact,
            VersionSpec::Partial(prefix) => {
                version.starts_with(prefix)
                    && (version.len() == prefix.len()
                        || !version.as_bytes()[prefix.len()].is_ascii_digit())
            }
        }
    }

    /// Returns `true` if `self` is compatible with `other`, in that there is at least a
    /// theoretical version that satisfies both.
    #[instrument]
    fn is_compatible(&self, other: &Self) -> bool {
        match (self, other) {
            (VersionSpec::Any, _) => true,
            (_, VersionSpec::Any) => true,
            (VersionSpec::Exact(a), VersionSpec::Exact(b)) => a == b,
            (VersionSpec::Exact(a), VersionSpec::Partial(_)) => other.matches(a),
            (VersionSpec::Partial(_), VersionSpec::Exact(b)) => self.matches(b),
            (VersionSpec::Partial(a), VersionSpec::Partial(b)) => {
                self.matches(b) || other.matches(a)
            }
        }
    }
}

impl Display for VersionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionSpec::Any => write!(f, "*"),
            VersionSpec::Exact(version) => write!(f, "{}", version),
            VersionSpec::Partial(prefix) => write!(f, "~{}", prefix),
        }
    }
}

impl BitAnd for VersionSpec {
    type Output = Option<Self>;

    #[instrument]
    fn bitand(self, rhs: Self) -> Self::Output {
        if self == rhs {
            return Some(self);
        }
        if !self.is_compatible(&rhs) {
            return None;
        }
        Some(match (self.clone(), rhs.clone()) {
            (VersionSpec::Any, _) => rhs,
            (_, VersionSpec::Any) => self,
            (VersionSpec::Exact(a), VersionSpec::Exact(_)) => VersionSpec::Exact(a),
            (VersionSpec::Exact(a), VersionSpec::Partial(_)) => VersionSpec::Exact(a),
            (VersionSpec::Partial(_), VersionSpec::Exact(b)) => VersionSpec::Exact(b),
            (VersionSpec::Partial(a), VersionSpec::Partial(b)) if a.len() <= b.len() => rhs,
            _ => self,
        })
    }
}

impl FromStr for VersionSpec {
    type Err = InvalidVersonSpec;

    #[instrument]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || s == "*" {
            return Ok(VersionSpec::Any);
        }
        if let Some(v) = s.strip_prefix('~') {
            return Ok(VersionSpec::partial(v));
        }
        Ok(VersionSpec::exact(s))
    }
}

impl TryFrom<String> for VersionSpec {
    type Error = InvalidVersonSpec;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

/// Attempts to merge a set of dependency requests in such a way that each dependency is only
/// present once, and the version spec for each dependency is the intersection of all the version
/// specs for that dependency.
#[instrument(skip(requests))]
fn merge_dependency_requests(
    requests: impl IntoIterator<Item = PackageRequest>,
) -> Result<Vec<PackageRequest>, Conflicts> {
    let mut rv: Vec<PackageRequest> = Vec::new();
    let mut conflicts: Conflicts = Conflicts::default();

    for request in requests {
        // New request, just add it.
        let Some(existing_request) = rv.iter_mut().find(|r| r.name == request.name) else {
            rv.push(request);
            continue;
        };

        // Existing compatible request, merge the version specs.
        if let Some(merged) = existing_request.version.clone() & request.version.clone() {
            existing_request.version = merged;
            continue;
        }

        // Incompatible request, either add a new conflict or add to an existing one.
        conflicts.add_conflict(
            request.name,
            existing_request.version.clone(),
            request.version,
        )
    }

    if conflicts.is_empty() {
        Ok(rv)
    } else {
        Err(conflicts)
    }
}

/// A [`PackageRequest`] with a resolved version based on known packages.
#[derive(Clone, Debug, FromRow)]
pub struct KnownPackage {
    /// The name of the package.
    pub name: String,
    /// The resolved version of the package.
    pub version: String,
}

impl KnownPackage {
    /// Creates a new spec from a [`crate::manifest::Package`].
    pub fn from_manifest_package(pkg: &Package) -> Self {
        Self {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
        }
    }

    pub fn from_request(request: &PackageRequest, version: &str) -> Self {
        Self {
            name: request.name.clone(),
            version: version.to_string(),
        }
    }
}

impl PackageSpec for KnownPackage {
    fn spec(&self) -> (String, String) {
        (self.name.clone(), self.version.clone())
    }
}

impl Display for KnownPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// Test-only crutch.
#[cfg(test)]
impl From<WorkspacePackage> for KnownPackage {
    fn from(spec: WorkspacePackage) -> Self {
        Self {
            name: spec.name,
            version: spec.version,
        }
    }
}

/// A [`PackageRequest`] with a resolved version based packages in a workspace.
#[derive(Clone, Debug, FromRow)]
pub struct WorkspacePackage {
    /// The name of the package.
    pub name: String,
    /// The resolved version of the package.
    pub version: String,
    /// The unresolved version that was requested.
    #[sqlx(try_from = "String")]
    pub requested_version: VersionSpec,
}

impl WorkspacePackage {
    pub fn from_request(request: &PackageRequest, version: &str) -> Self {
        Self {
            name: request.name.clone(),
            version: version.to_string(),
            requested_version: request.version.clone(),
        }
    }

    /// Returns the latest known version of this package, if it is newer than the installed one.
    #[instrument(skip(state))]
    pub async fn available_update(&self, state: &State) -> Result<Option<KnownPackage>> {
        let known_versions = state.known_package_versions(&self.name).await?;
        let Some(latest) = known_versions
            .into_iter()
            .find(|v| self.requested_version.matches(v))
        else {
            return Ok(None);
        };
        if self.version < latest {
            Ok(Some(KnownPackage {
                name: self.name.clone(),
                version: latest,
            }))
        } else {
            Ok(None)
        }
    }

    /// Removes this package's files from a workspace.
    #[instrument]
    pub async fn remove(&self, workspace: &Workspace) -> Result<()> {
        workspace
            .remove_package(self)
            .await
            .wrap_err("failed to remove package from workspace")
    }
}

impl PackageSpec for WorkspacePackage {
    fn spec(&self) -> (String, String) {
        (self.name.clone(), self.version.clone())
    }
}

impl Display for WorkspacePackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}@{} (resolved from {})",
            self.name, self.version, self.requested_version
        )
    }
}

// Test-only crutch.
#[cfg(test)]
impl From<KnownPackage> for WorkspacePackage {
    fn from(spec: KnownPackage) -> Self {
        Self {
            name: spec.name,
            version: spec.version,
            requested_version: VersionSpec::Any,
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
    #[instrument]
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

impl From<&WorkspacePackage> for InstalledPackage {
    fn from(spec: &WorkspacePackage) -> Self {
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
    use std::collections::HashSet;

    use crate::{
        manifest::Package as ManifestPackage,
        registry::{MockFetcher, Registry},
        workspace::test_workspace,
    };

    use super::*;

    #[test]
    fn test_matches_any_version() {
        assert!(VersionSpec::Any.matches("1.0.0"));
        assert!(VersionSpec::Any.matches("0.0.0"));
        assert!(VersionSpec::Any.matches("0.1.0"));
        assert!(VersionSpec::Any.matches("0.0.1"));
        assert!(VersionSpec::Any.matches("0.1.1"));
        assert!(VersionSpec::Any.matches("1.1.1"));
    }

    #[test]
    fn test_matches_exact_version() {
        assert!(VersionSpec::exact("1.0.0").matches("1.0.0"));
        assert!(!VersionSpec::exact("1.0.0").matches("1.0"));
        assert!(!VersionSpec::exact("1.0.0").matches("1.0.1"));
        assert!(!VersionSpec::exact("1.0.0").matches("1.0.0-beta"));
    }

    #[test]
    fn test_matches_partial_version() {
        assert!(VersionSpec::partial("1").matches("1"));
        assert!(VersionSpec::partial("1").matches("1.0"));
        assert!(VersionSpec::partial("1").matches("1.0.0"));
        assert!(VersionSpec::partial("1").matches("1.1"));

        assert!(!VersionSpec::partial("1").matches("10"));
        assert!(VersionSpec::partial("1").matches("1-alpha2"));
    }

    #[test]
    fn test_is_compatible_any() {
        assert!(VersionSpec::Any.is_compatible(&VersionSpec::Any));
        assert!(VersionSpec::Any.is_compatible(&VersionSpec::exact("1.0.0")));
        assert!(VersionSpec::Any.is_compatible(&VersionSpec::partial("1")));
        assert!(VersionSpec::exact("1.0.0").is_compatible(&VersionSpec::Any));
        assert!(VersionSpec::partial("1").is_compatible(&VersionSpec::Any));
    }

    #[test]
    fn test_is_compatible_exact() {
        assert!(VersionSpec::exact("1.0.0").is_compatible(&VersionSpec::exact("1.0.0")));
        assert!(!VersionSpec::exact("1.0.0").is_compatible(&VersionSpec::exact("1.0.1")));
    }

    #[test]
    fn test_is_compatible_partial_exact() {
        assert!(VersionSpec::partial("1").is_compatible(&VersionSpec::exact("1.0.0")));
        assert!(VersionSpec::exact("1.0.0").is_compatible(&VersionSpec::partial("1")));
        assert!(!VersionSpec::partial("1").is_compatible(&VersionSpec::exact("2.0.0")));
        assert!(!VersionSpec::exact("2.0.0").is_compatible(&VersionSpec::partial("1")));

        assert!(!VersionSpec::partial("1").is_compatible(&VersionSpec::exact("12.0.0")));
        assert!(!VersionSpec::exact("12.0.0").is_compatible(&VersionSpec::partial("1")));
    }

    #[test]
    fn test_is_compatible_partial_partial() {
        assert!(VersionSpec::partial("1").is_compatible(&VersionSpec::partial("1")));
        assert!(VersionSpec::partial("1").is_compatible(&VersionSpec::partial("1.1")));
        assert!(VersionSpec::partial("1.1").is_compatible(&VersionSpec::partial("1")));

        assert!(!VersionSpec::partial("1.2").is_compatible(&VersionSpec::partial("1.1")));
    }

    #[test]
    fn test_bit_add_version_specs() {
        assert_eq!(VersionSpec::Any & VersionSpec::Any, Some(VersionSpec::Any));
        assert_eq!(
            VersionSpec::Any & VersionSpec::exact("1.0.0"),
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            VersionSpec::Any & VersionSpec::partial("1"),
            Some(VersionSpec::partial("1"))
        );
        assert_eq!(
            VersionSpec::exact("1.0.0") & VersionSpec::Any,
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            VersionSpec::partial("1") & VersionSpec::Any,
            Some(VersionSpec::partial("1"))
        );

        assert_eq!(
            VersionSpec::exact("1.0.0") & VersionSpec::exact("1.0.0"),
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            VersionSpec::exact("1.0.0") & VersionSpec::exact("1.0.1"),
            None
        );

        assert_eq!(
            VersionSpec::partial("1") & VersionSpec::exact("1.0.0"),
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            VersionSpec::exact("1.0.0") & VersionSpec::partial("1"),
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            VersionSpec::partial("1") & VersionSpec::partial("1.0"),
            Some(VersionSpec::partial("1.0"))
        );
        assert_eq!(
            VersionSpec::partial("1.0") & VersionSpec::partial("1"),
            Some(VersionSpec::partial("1.0"))
        );

        assert_eq!(VersionSpec::partial("1") & VersionSpec::partial("2"), None);
    }

    #[test]
    fn test_parse_version_spec() {
        assert_eq!(VersionSpec::from_str("").unwrap(), VersionSpec::Any);
        assert_eq!(VersionSpec::from_str("*").unwrap(), VersionSpec::Any);
        assert_eq!(
            VersionSpec::from_str("1.0.0").unwrap(),
            VersionSpec::exact("1.0.0")
        );
        assert_eq!(
            VersionSpec::from_str("~1.0.0").unwrap(),
            VersionSpec::partial("1.0.0")
        );
    }

    #[test]
    fn test_merge_dependency_requests_all_any() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                }
            ])?,
            vec![PackageRequest {
                name: "foo".into(),
                version: VersionSpec::Any
            }]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_any_exact() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                }
            ])?,
            vec![PackageRequest {
                name: "foo".into(),
                version: VersionSpec::exact("1.0.0")
            }]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_any_partial() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                }
            ])?,
            vec![PackageRequest {
                name: "foo".into(),
                version: VersionSpec::partial("1")
            }]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_matching_partials() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1.0")
                }
            ])?,
            vec![PackageRequest {
                name: "foo".into(),
                version: VersionSpec::partial("1.0")
            }]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_mismatching_partials() {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("2")
                }
            ]),
            Err(Conflicts {
                inner: vec![(
                    "foo".into(),
                    HashSet::from([VersionSpec::partial("1"), VersionSpec::partial("2")])
                )]
            })
        );
    }

    #[test]
    fn test_merge_dependency_requests_partial_exact() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                }
            ])?,
            vec![PackageRequest {
                name: "foo".into(),
                version: VersionSpec::exact("1.0.0")
            }]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_exact_mismatch() {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.1")
                }
            ]),
            Err(Conflicts {
                inner: vec![(
                    "foo".into(),
                    HashSet::from([VersionSpec::exact("1.0.0"), VersionSpec::exact("1.0.1")])
                )]
            })
        );
    }

    #[test]
    fn test_merge_dependency_requests_matching_exact() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                }
            ])?,
            vec![PackageRequest {
                name: "foo".into(),
                version: VersionSpec::exact("1.0.0")
            }]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_different_names() -> Result<()> {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                PackageRequest {
                    name: "bar".into(),
                    version: VersionSpec::exact("2.0.0")
                }
            ])?,
            vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                PackageRequest {
                    name: "bar".into(),
                    version: VersionSpec::exact("2.0.0")
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn test_merge_dependency_requests_triple_conflict() {
        assert_eq!(
            merge_dependency_requests(vec![
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.1")
                },
                PackageRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.2")
                }
            ]),
            Err(Conflicts {
                inner: vec![(
                    "foo".into(),
                    HashSet::from([
                        VersionSpec::exact("1.0.0"),
                        VersionSpec::exact("1.0.1"),
                        VersionSpec::exact("1.0.2")
                    ])
                )]
            })
        );
    }

    #[test]
    fn test_dependency_request_parse_round_trip() -> Result<()> {
        let exact = "foo@1.0.0";
        assert_eq!(format!("{}", exact.parse::<PackageRequest>()?), exact);

        let partial = "foo@~1.0.0";
        assert_eq!(format!("{}", partial.parse::<PackageRequest>()?), partial);

        let any = "foo";
        assert_eq!(format!("{}", any.parse::<PackageRequest>()?), any);

        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_known_version() -> Result<()> {
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
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            }])
            .await
            .unwrap();
        let pkg: PackageRequest = "foo".parse()?;
        let spec = pkg.resolve_known_version(&state).await.unwrap();
        assert_eq!(spec.version, "1.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_known_version_fails_if_not_known() -> Result<()> {
        let state = State::load(":memory:").await.unwrap();
        let pkg: PackageRequest = "foo".parse()?;
        assert!(pkg.resolve_known_version(&state).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_known_version_fails_if_this_version_is_not_known() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, _workspace) = test_workspace("global").await;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();

        let known_package = Package {
            name: "foo".into(),
            version: "1.0.0".into(),
            registry: Some("https://example.invalid/registry".into()),
            ..Default::default()
        };
        state.add_known_packages(&[known_package]).await?;

        let pkg: PackageRequest = "foo@2.0.0".parse()?;
        assert!(pkg.resolve_known_version(&state).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_workspace_version() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (workspace, _workspace_root) = test_workspace("global").await;

        let req = "foo@1.0.0".parse()?;
        let known_package = KnownPackage::from_request(&req, "1.0.0");
        let workspace_package = WorkspacePackage::from_request(&req, "1.0.0");

        state.add_installed_package(&known_package).await?;
        state
            .add_workspace_package(&workspace_package, &workspace)
            .await?;

        let pkg: PackageRequest = "foo".parse()?;
        let spec = pkg.resolve_workspace_version(&state, &workspace).await?;
        assert_eq!(spec.version, "1.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_workspace_version_fails_if_not_installed() -> Result<()> {
        let state = State::load(":memory:").await.unwrap();
        let (workspace, _workspace_root) = test_workspace("global").await;
        let pkg: PackageRequest = "foo".parse()?;
        assert!(pkg
            .resolve_workspace_version(&state, &workspace)
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_workspace_version_fails_if_this_version_is_not_installed() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (workspace, _workspace_root) = test_workspace("global").await;

        let req: PackageRequest = "foo@1".parse()?;
        let known_package = KnownPackage::from_request(&req, "1.0.0");
        let workspace_package = WorkspacePackage::from_request(&req, "1.0.0");

        state.add_installed_package(&known_package).await?;

        state
            .add_workspace_package(&workspace_package, &workspace)
            .await?;
        let pkg: PackageRequest = "foo@2".parse()?;
        assert!(pkg
            .resolve_workspace_version(&state, &workspace)
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_changeset_resolve_add_new_in_vacuum() -> Result<()> {
        let changeset = PackageChangeSet::add_packages(&["foo@1.0.0".parse()?], &[])?;

        let added = changeset.added_packages().collect::<Vec<_>>();
        assert_eq!(added.len(), 1);
        assert!(added.contains(&"foo@1.0.0".parse()?));

        Ok(())
    }

    #[tokio::test]
    async fn test_changeset_resolve_add_new_with_unrelated() -> Result<()> {
        let changeset = PackageChangeSet::add_packages(
            &["foo@1.0.0".parse()?],
            &[WorkspacePackage::from_request(
                &"bar".parse::<PackageRequest>()?,
                "1.0.0",
            )],
        )?;

        let added = changeset.added_packages().collect::<Vec<_>>();
        assert_eq!(added.len(), 1);
        assert!(added.contains(&"foo@1.0.0".parse()?));

        Ok(())
    }

    #[tokio::test]
    async fn test_changeset_resolve_add_new_preexisting_upgrades() -> Result<()> {
        let changeset = PackageChangeSet::add_packages(
            &["foo".parse()?],
            &[WorkspacePackage::from_request(
                &"foo@1".parse::<PackageRequest>()?,
                "1.0.0",
            )],
        )?;

        let changed = changeset.changed_packages().collect::<Vec<_>>();
        assert_eq!(changed.len(), 1);
        assert!(changed.contains(&"foo@1".parse()?));

        Ok(())
    }

    #[tokio::test]
    async fn test_changeset_resolve_add_new_preexisting_conflicts() -> Result<()> {
        let changeset = PackageChangeSet::add_packages(
            &["foo@2".parse()?],
            &[WorkspacePackage::from_request(
                &"foo@1".parse::<PackageRequest>()?,
                "1",
            )],
        );

        assert!(changeset.unwrap_err().to_string().contains("conflict"));

        Ok(())
    }

    #[tokio::test]
    async fn test_changeset_resolve_add_new_preexisting_needs_change() -> Result<()> {
        let changeset = PackageChangeSet::add_packages(
            &["foo@2".parse()?],
            &[WorkspacePackage::from_request(
                &"foo".parse::<PackageRequest>()?,
                "1",
            )],
        )?;

        let changed = changeset.changed_packages().collect::<Vec<_>>();
        assert_eq!(changed.len(), 1);
        assert!(changed.contains(&"foo@2".parse()?));

        Ok(())
    }

    #[tokio::test]
    async fn test_changeset_resolve_add_new_preexisting_lax_needs_change() -> Result<()> {
        let changeset = PackageChangeSet::add_packages(
            &["foo@1.1".parse()?],
            &[WorkspacePackage::from_request(
                &"foo@~1".parse::<PackageRequest>()?,
                "1.0",
            )],
        )?;

        let changed = changeset.changed_packages().collect::<Vec<_>>();
        assert_eq!(changed.len(), 1);
        assert!(changed.contains(&"foo@1.1".parse()?));

        Ok(())
    }
}

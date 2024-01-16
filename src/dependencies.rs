use std::{fmt::Display, ops::BitAnd, str::FromStr};

use anyhow::{anyhow, Result};

use crate::{
    error::{Conflicts, InvalidVersonSpec},
    package::{KnownPackageSpec, PackageRequest, PackageSpec, WorkspacePackageSpec},
    state::State,
    workspace::Workspace,
};

/// A set of changes to workspace packages.
#[derive(Debug, Default)]
pub struct PackageChangeSet {
    /// Packages that need to be added.
    add: Vec<DependencyRequest>,
    /// Packages that need to be upgraded or downgraded.
    change: Vec<DependencyRequest>,
    /// Packages that need to be removed.
    remove: Vec<DependencyRequest>,
}

impl PackageChangeSet {
    /// Creates a changeset that adds the given packages.
    pub fn add_packages(
        pkgs: &[DependencyRequest],
        workspace_packages: &[WorkspacePackageSpec],
    ) -> Result<Self> {
        let mut change_set = Self {
            add: Vec::from(pkgs),
            ..Self::default()
        };

        change_set.resolve(workspace_packages)?;

        Ok(change_set)
    }

    /// Creates a changeset that updates the given packages.
    pub fn update_packages(
        pkgs: &[DependencyRequest],
        workspace_packages: &[WorkspacePackageSpec],
    ) -> Result<Self> {
        let mut change_set = Self {
            change: Vec::from(pkgs),
            ..Self::default()
        };

        change_set.resolve(workspace_packages)?;

        Ok(change_set)
    }

    /// Creates a changeset that removes the given packages.
    pub fn remove_packages(
        pkgs: &[DependencyRequest],
        workspace_packages: &[WorkspacePackageSpec],
    ) -> Result<Self> {
        let mut change_set = Self {
            remove: Vec::from(pkgs),
            ..Self::default()
        };

        change_set.resolve(workspace_packages)?;

        Ok(change_set)
    }

    /// Returns the packages that need to be added.
    pub fn added_packages(&self) -> impl Iterator<Item = DependencyRequest> + '_ {
        self.add.iter().cloned()
    }

    /// Returns the packages that need to be upgraded or downgraded.
    pub fn changed_packages(&self) -> impl Iterator<Item = DependencyRequest> + '_ {
        self.change.iter().cloned()
    }

    /// Returns the packages that need to be removed.
    pub fn removed_packages(&self) -> impl Iterator<Item = DependencyRequest> + '_ {
        self.remove.iter().cloned()
    }

    /// Resolves the changeset based on the current workflow packages.
    fn resolve(&mut self, current: &[WorkspacePackageSpec]) -> Result<()> {
        // Get all the requests currently in the workspace.
        let current_requests = current
            .iter()
            .map(|p| p.to_owned().into())
            .collect::<Vec<DependencyRequest>>();

        // Merge the current requests with the new requests.
        let merged_requests =
            merge_dependency_requests(current_requests.into_iter().chain(self.add.clone()))?;

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
                self.add.push(request);
            }
        }

        Ok(())
    }
}

/// A dependency of a package, with an unresolved version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DependencyRequest {
    /// The name of the dependency.
    pub name: String,
    /// The version of the dependency.
    pub version: VersionSpec,
}

impl DependencyRequest {
    /// Resolves this request to a known package that can be installed.
    ///
    /// If the version isn't fully qualified, resolves it to the latest known one. Returns an error
    /// if the package is not known. If multiple versions of the package are known, the first
    /// (latest) one that matches is used.
    pub async fn resolve_known_version(&self, state: &State) -> Result<KnownPackageSpec> {
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

        Ok(KnownPackageSpec::from_request(self, resolved))
    }

    /// Resolves this request to a workspace package from the given workspace.
    ///
    /// If the version isn't fully qualified, resolves it to the latest installed one. Returns an
    /// error if the package is not installed in this workspace.
    pub async fn resolve_workspace_version(
        &self,
        state: &State,
        workspace: &Workspace,
    ) -> Result<WorkspacePackageSpec> {
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

        Ok(WorkspacePackageSpec::from_request(self, &installed.version))
    }
}

impl FromStr for DependencyRequest {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '@');
        let Some(name) = parts.next() else {
            anyhow::bail!("invalid dependency request: {}", s);
        };
        let version = parts.next().unwrap_or("*");
        Ok(Self {
            name: name.into(),
            version: version.parse()?,
        })
    }
}

impl From<PackageRequest> for DependencyRequest {
    fn from(value: PackageRequest) -> Self {
        Self {
            name: value.name,
            version: match value.version {
                Some(v) => v.parse().expect("invalid version spec"),
                None => VersionSpec::Any,
            },
        }
    }
}

impl From<WorkspacePackageSpec> for DependencyRequest {
    fn from(value: WorkspacePackageSpec) -> Self {
        Self {
            name: value.name,
            version: value.version.parse().expect("invalid version spec"),
        }
    }
}

impl Display for DependencyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let VersionSpec::Any = self.version {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}@{}", self.name, self.version)
        }
    }
}

impl PackageSpec for DependencyRequest {
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

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "*" {
            return Ok(VersionSpec::Any);
        }
        if let Some(v) = s.strip_prefix('~') {
            return Ok(VersionSpec::partial(v));
        }
        Ok(VersionSpec::exact(s))
    }
}

/// Attempts to merge a set of dependency requests in such a way that each dependency is only
/// present once, and the version spec for each dependency is the intersection of all the version
/// specs for that dependency.
fn merge_dependency_requests(
    requests: impl IntoIterator<Item = DependencyRequest>,
) -> Result<Vec<DependencyRequest>, Conflicts> {
    let mut rv: Vec<DependencyRequest> = Vec::new();
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

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use anyhow::Result;

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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                }
            ])?,
            vec![DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                }
            ])?,
            vec![DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                }
            ])?,
            vec![DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1.0")
                }
            ])?,
            vec![DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                },
                DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::partial("1")
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                }
            ])?,
            vec![DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::Any
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                }
            ])?,
            vec![DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                DependencyRequest {
                    name: "bar".into(),
                    version: VersionSpec::exact("2.0.0")
                }
            ])?,
            vec![
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                DependencyRequest {
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
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.0")
                },
                DependencyRequest {
                    name: "foo".into(),
                    version: VersionSpec::exact("1.0.1")
                },
                DependencyRequest {
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
        assert_eq!(format!("{}", exact.parse::<DependencyRequest>()?), exact);

        let partial = "foo@~1.0.0";
        assert_eq!(
            format!("{}", partial.parse::<DependencyRequest>()?),
            partial
        );

        let any = "foo";
        assert_eq!(format!("{}", any.parse::<DependencyRequest>()?), any);

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
        let pkg: DependencyRequest = "foo".parse()?;
        let spec = pkg.resolve_known_version(&state).await.unwrap();
        assert_eq!(spec.version, "1.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_known_version_fails_if_not_known() -> Result<()> {
        let state = State::load(":memory:").await.unwrap();
        let pkg: DependencyRequest = "foo".parse()?;
        assert!(pkg.resolve_known_version(&state).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_known_version_fails_if_this_version_is_not_known() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = KnownPackageSpec {
            name: "foo".into(),
            version: "1.0.0".into(),
            requested_version: "1.0.0".into(),
        };

        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let pkg: DependencyRequest = "foo@2.0.0".parse()?;
        assert!(pkg.resolve_known_version(&state).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_workspace_version() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = KnownPackageSpec {
            name: "foo".into(),
            version: "1.0.0".into(),
            requested_version: "1.0.0".into(),
        };
        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let pkg: DependencyRequest = "foo".parse()?;
        let spec = pkg.resolve_workspace_version(&state, &workspace).await?;
        assert_eq!(spec.version, "1.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_workspace_version_fails_if_not_installed() -> Result<()> {
        let state = State::load(":memory:").await.unwrap();
        let (_root, workspace) = test_workspace("global").await;
        let pkg: DependencyRequest = "foo".parse()?;
        assert!(pkg
            .resolve_workspace_version(&state, &workspace)
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_workspace_version_fails_if_this_version_is_not_installed() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = KnownPackageSpec {
            name: "foo".into(),
            version: "1.0.0".into(),
            requested_version: "1.0.0".into(),
        };
        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let pkg: DependencyRequest = "foo@2".parse()?;
        assert!(pkg
            .resolve_workspace_version(&state, &workspace)
            .await
            .is_err());
        Ok(())
    }
}

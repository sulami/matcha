use std::{collections::HashSet, fmt::Display, ops::BitAnd, str::FromStr};

/// A dependency of a package, with an unresolved version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DependencyRequest {
    /// The name of the dependency.
    pub name: String,
    /// The version of the dependency.
    pub version: VersionSpec,
}

/// A version spec, which can be used to resolve to a concrete version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VersionSpec {
    /// Any version at all.
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

    /// Returns `true` if `self` is compatible with `other`.
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
    type Err = ();

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

/// Conflicts between dependency requests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Conflicts {
    /// The conflicting dependency requests.
    inner: Vec<(String, HashSet<VersionSpec>)>,
}

impl Conflicts {
    /// Adds a conflict.
    fn add_conflict(&mut self, name: String, a: VersionSpec, b: VersionSpec) {
        if let Some((_, versions)) = self.inner.iter_mut().find(|(n, _)| n == &name) {
            versions.insert(a);
            versions.insert(b);
        } else {
            self.inner.push((name, HashSet::from([a, b])));
        }
    }

    /// Returns true if there are no conflicts.
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Display for Conflicts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, requests) in &self.inner {
            writeln!(f, "conflicting requests for dependency '{}':", name)?;
            for request in requests {
                writeln!(f, "  {:?}", request)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for Conflicts {}

/// Attempts to merge a set of dependency requests in such a way that each dependency is only
/// present once, and the version spec for each dependency is the intersection of all the version
/// specs for that dependency.
pub fn merge_dependency_requests(
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
    use anyhow::Result;

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
}

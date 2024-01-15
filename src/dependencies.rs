use std::{ops::BitAnd, str::FromStr};

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

/// Attempts to merge a set of dependency requests into a single request that satisfies all of
/// them.
fn merge_version_specs(specs: Vec<VersionSpec>) -> Option<VersionSpec> {
    let mut rv = VersionSpec::Any;

    for spec in specs {
        if let Some(merged) = rv & spec {
            rv = merged;
        } else {
            return None;
        }
    }

    Some(rv)
}

#[cfg(test)]
mod test {
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
    fn test_merge_version_specs() {
        assert_eq!(
            merge_version_specs(vec![VersionSpec::Any, VersionSpec::Any]),
            Some(VersionSpec::Any)
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::exact("1.0.0"),
                VersionSpec::Any
            ]),
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::exact("1.0.0"),
                VersionSpec::exact("1.0.1")
            ]),
            None
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::partial("1"),
                VersionSpec::Any
            ]),
            Some(VersionSpec::partial("1"))
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::partial("1"),
                VersionSpec::partial("1.0")
            ]),
            Some(VersionSpec::partial("1.0"))
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::partial("1"),
                VersionSpec::partial("2")
            ]),
            None
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::partial("1"),
                VersionSpec::exact("1.0.0")
            ]),
            Some(VersionSpec::exact("1.0.0"))
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::partial("1"),
                VersionSpec::exact("2.0.0")
            ]),
            None
        );
        assert_eq!(
            merge_version_specs(vec![
                VersionSpec::Any,
                VersionSpec::exact("1.0.0"),
                VersionSpec::exact("1.0.0")
            ]),
            Some(VersionSpec::exact("1.0.0"))
        );
    }
}

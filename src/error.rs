use std::{collections::HashSet, error::Error, fmt::Display};

use crate::package::VersionSpec;

/// An invalid version spec was encountered.
#[derive(Debug, Clone)]
pub struct InvalidVersonSpec(String);
impl Error for InvalidVersonSpec {}
impl Display for InvalidVersonSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid version spec: {}", self.0)
    }
}

/// Conflicts between dependency requests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Conflicts {
    /// The conflicting dependency requests.
    pub inner: Vec<(String, HashSet<VersionSpec>)>,
}

impl Conflicts {
    /// Adds a conflict.
    pub fn add_conflict(&mut self, name: String, a: VersionSpec, b: VersionSpec) {
        if let Some((_, versions)) = self.inner.iter_mut().find(|(n, _)| n == &name) {
            versions.insert(a);
            versions.insert(b);
        } else {
            self.inner.push((name, HashSet::from([a, b])));
        }
    }

    /// Returns true if there are no conflicts.
    pub fn is_empty(&self) -> bool {
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

impl Error for Conflicts {}

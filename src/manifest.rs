use std::str::FromStr;

use anyhow::Error;
use serde::Deserialize;

/// Manifest metadata.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// The schema version of the manifest.
    pub schema_version: u32,
    /// The name of the manifest.
    pub name: String,
    /// The URI of the manifest.
    pub uri: String,
    /// The version of the manifest.
    pub version: String,
    /// The description of the manifest.
    pub description: Option<String>,
    /// Packages in this manifest.
    pub packages: Vec<Package>,
}

/// A package, as described by a package manifest.
#[derive(Debug, Deserialize)]
pub struct Package {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: String,
    /// The description of the package.
    pub description: Option<String>,
    /// The homepage of the package.
    pub homepage: Option<String>,
    /// The license of the package.
    pub license: Option<String>,
}

impl FromStr for Manifest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(toml::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest() {
        let manifest = r#"
            schema_version = 1
            name = "test"
            uri = "https://example.invalid/test"
            version = "0.1.0"
            description = "A test manifest"

            [[packages]]
            name = "test-package"
            version = "0.1.0"
            description = "A test package"
            homepage = "https://example.invalid/test-package"
            license = "MIT"
        "#;

        let manifest: Manifest = manifest.parse().unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.name, "test");
        assert_eq!(manifest.uri, "https://example.invalid/test");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.description, Some("A test manifest".to_string()));
        assert_eq!(manifest.packages.len(), 1);
        assert_eq!(manifest.packages[0].name, "test-package");
        assert_eq!(manifest.packages[0].version, "0.1.0");
        assert_eq!(
            manifest.packages[0].description,
            Some("A test package".to_string())
        );
        assert_eq!(
            manifest.packages[0].homepage,
            Some("https://example.invalid/test-package".to_string())
        );
        assert_eq!(manifest.packages[0].license, Some("MIT".to_string()));
    }
}

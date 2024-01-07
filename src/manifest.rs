use std::{fmt::Display, str::FromStr};

use anyhow::{anyhow, Context, Error, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Deserializer};
use sqlx::{types::Json, FromRow};
use tempfile::TempDir;
use tokio::{fs::File, io::AsyncWriteExt, process::Command};
use url::Url;

use crate::{download::download_stream, workspace::Workspace};

/// Manifest metadata.
#[derive(Debug, Default)]
pub struct Manifest {
    /// The schema version of the manifest.
    pub schema_version: u32,
    /// The name of the manifest.
    pub name: String,
    /// The URI of the manifest.
    pub uri: String,
    /// The description of the manifest.
    pub description: Option<String>,
    /// Packages in this manifest.
    pub packages: Vec<Package>,
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TempPackage {
            name: String,
            version: String,
            description: Option<String>,
            homepage: Option<String>,
            license: Option<String>,
            source: Option<String>,
            build: Option<String>,
            artifacts: Option<Vec<String>>,
        }

        #[derive(Deserialize)]
        struct TempManifest {
            schema_version: u32,
            name: String,
            uri: String,
            description: Option<String>,
            packages: Vec<TempPackage>,
        }

        let temp_manifest = TempManifest::deserialize(deserializer)?;

        let packages = temp_manifest
            .packages
            .into_iter()
            .map(|temp_package| Package {
                name: temp_package.name,
                version: temp_package.version,
                description: temp_package.description,
                homepage: temp_package.homepage,
                license: temp_package.license,
                registry: temp_manifest.name.clone(),
                source: temp_package.source,
                build: temp_package.build,
                artifacts: Json(temp_package.artifacts),
            })
            .collect();

        Ok(Manifest {
            schema_version: temp_manifest.schema_version,
            name: temp_manifest.name,
            uri: temp_manifest.uri,
            description: temp_manifest.description,
            packages,
        })
    }
}

impl FromStr for Manifest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(toml::from_str(s)?)
    }
}

/// A package, as described by a registry manifest.
#[derive(Debug, FromRow, Deserialize, Default)]
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
    /// The source of the package. Can be `None` for meta packages.
    pub source: Option<String>,
    /// The build command of the package.
    pub build: Option<String>,
    /// The artifacts of the package after building.
    pub artifacts: Json<Option<Vec<String>>>,
    /// The registry this package is from.
    pub registry: String,
}

impl Package {
    /// Downloads and builds the package.
    pub async fn build(&self, workspace: &Workspace) -> Result<()> {
        let Some(source) = &self.source else {
            // Nothing to do here.
            return Ok(());
        };

        let source = Url::parse(source).context("invalid source URL")?;

        // Create a temporary working directory.
        let temp_dir = TempDir::new()?;

        // Stream the download to a file.
        let (_size, mut download) = download_stream(source.as_str()).await?;
        let download_file_name = source
            .path_segments()
            .ok_or(anyhow!("invalid package download source"))?
            .last()
            .unwrap_or("download");
        let mut file = File::create(temp_dir.path().join(download_file_name)).await?;
        while let Some(chunk) = download.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
        }

        // Perform build steps, if any.
        if let Some(build) = &self.build {
            let output = Command::new("zsh")
                .arg("-c")
                .arg(build)
                .current_dir(temp_dir.path())
                .spawn()
                .context("failed to spawn build command")?
                .wait_with_output()
                .await?;

            if !output.status.success() {
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
                eprint!("{}", String::from_utf8_lossy(&output.stdout));
                return Err(anyhow!(
                    "build command exited with non-zero status code: {}",
                    output.status,
                ));
            }
        }

        // Copy artifacts to the workspace.
        if let Some(artifacts) = &*self.artifacts {
            for artifact in artifacts {
                if artifact.starts_with('/') {
                    return Err(anyhow!("artifact path cannot be absolute"));
                }
                let artifact = temp_dir.path().join(artifact);
                if let Some(parent) = artifact.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::copy(
                    &artifact,
                    workspace.path()?.join(artifact.file_name().unwrap()),
                )
                .await?;
            }
        }

        Ok(())
    }
}

impl Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)?;
        if let Some(desc) = &self.description {
            write!(f, " - {}", desc)?;
        }
        if let Some(url) = &self.homepage {
            write!(f, " ({})", url)?;
        }
        if let Some(license) = &self.license {
            write!(f, " [{}]", license)?;
        }
        write!(f, " < {}", self.registry)
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
            description = "A test manifest"

            [[packages]]
            name = "test-package"
            version = "0.1.0"
            description = "A test package"
            homepage = "https://example.invalid/test-package"
            license = "MIT"
            source = "https://example.invalid/test-package/archive/0.1.0.tar.gz"
            build = "cargo build --release"
            artifacts = ["target/release/test-package"]
        "#;

        let manifest: Manifest = manifest.parse().unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.name, "test");
        assert_eq!(manifest.uri, "https://example.invalid/test");
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
        assert_eq!(manifest.packages[0].registry, "test");
        assert_eq!(
            manifest.packages[0].source,
            Some("https://example.invalid/test-package/archive/0.1.0.tar.gz".to_string())
        );
        assert_eq!(
            manifest.packages[0].build,
            Some("cargo build --release".to_string())
        );
    }
}

use std::{
    fmt::Display,
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
};

use anyhow::{anyhow, Context, Error, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::FromRow;
use tempfile::TempDir;
use tokio::{
    fs::{create_dir_all, metadata, read_dir, rename, symlink, File},
    io::AsyncWriteExt,
    pin,
    process::Command,
};
use url::Url;

use crate::{
    download::{DefaultDownloader, Downloader},
    package::KnownPackageSpec,
    state::State,
    workspace::Workspace,
    PACKAGE_ROOT,
};

/// Manifest metadata.
#[derive(Debug, Default, Serialize)]
pub struct Manifest {
    /// The schema version of the manifest.
    pub schema_version: u32,
    /// The name of the manifest.
    pub name: String,
    /// The URI of the registry this manifest is from.
    #[serde(skip)]
    pub uri: Option<String>,
    /// The description of the manifest.
    pub description: Option<String>,
    /// Packages in this manifest.
    pub packages: Vec<Package>,
}

impl Manifest {
    /// Sets the URI of the registry this manifest is from.
    pub fn set_registry_uri(&mut self, uri: &str) {
        self.uri = Some(uri.to_string());
        for package in &mut self.packages {
            package.registry = Some(uri.to_string());
        }
    }

    /// Returns whether this manifest is tied to a registry.
    ///
    /// If this is false, we have encountered a bug, because all manifests should come from a
    /// registry.
    pub fn is_tied_to_registry(&self) -> bool {
        self.uri.is_some() && self.packages.iter().all(|p| p.is_tied_to_registry())
    }
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
        }

        #[derive(Deserialize)]
        struct TempManifest {
            schema_version: u32,
            name: String,
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
                source: temp_package.source,
                build: temp_package.build,
                ..Default::default()
            })
            .collect();

        Ok(Manifest {
            schema_version: temp_manifest.schema_version,
            name: temp_manifest.name,
            description: temp_manifest.description,
            packages,
            ..Default::default()
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
#[derive(Clone, Debug, PartialEq, Eq, FromRow, Serialize, Deserialize, Default)]
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
    /// The registry this package is from.
    #[serde(skip)]
    pub registry: Option<String>,
}

impl Package {
    /// Returns whether this package is tied to a registry.
    ///
    /// If this is false, we have encountered a bug, because all packages should come from a
    /// registry.
    pub fn is_tied_to_registry(&self) -> bool {
        self.registry.is_some()
    }
}

/// The install log of a package, i.e. a report of the installation.
#[derive(Debug)]
pub struct InstallLog {
    /// The package this log is for.
    pub package_name: String,
    /// The exit code of the build.
    pub exit_code: i32,
    /// The stdout of the build.
    pub stdout: String,
    /// The stderr of the build.
    pub stderr: String,
    /// Whether this package was freshly installed.
    pub new_install: bool,
}

impl InstallLog {
    /// Creates a new install log for this package.
    fn new(package: &Package) -> Self {
        Self {
            package_name: format!("{package}"),
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            new_install: false,
        }
    }

    /// Returns whether the build was successful.
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}

impl Package {
    /// Downloads, builds, and installs the package.
    pub async fn install(&self, state: &State, workspace: &Workspace) -> Result<InstallLog> {
        if let Some(installed_package) = state
            .get_installed_package(&KnownPackageSpec::from_manifest_package(self))
            .await?
        {
            self.add_to_workspace(&installed_package.directory(), workspace)
                .await?;
            Ok(InstallLog::new(self))
        } else {
            let (build_dir, download_file_name) = self.download_source(&DefaultDownloader).await?;
            let (output_dir, log) = self.build(&build_dir, &download_file_name).await?;
            let pkg_dir = self.add_to_package_directory(&output_dir).await?;
            self.add_to_workspace(&pkg_dir, workspace).await?;
            Ok(log)
        }
    }

    /// Downloads the package source to a temporary build directory.
    ///
    /// Returns the build directory and the name of the downloaded file.
    async fn download_source(&self, downloader: &impl Downloader) -> Result<(TempDir, String)> {
        let build_dir = TempDir::new().context("failed to create build directory")?;

        // Download the package source, if any.
        let mut download_file_name = String::new();
        if let Some(source) = &self.source {
            let source = Url::parse(source).context("invalid source URL")?;

            // Stream the download to a file.
            let (_size, download) = downloader.download_stream(source.as_str()).await?;
            pin!(download);
            download_file_name = source
                .path_segments()
                .ok_or(anyhow!("invalid package download source"))?
                .last()
                .unwrap_or("matcha_download")
                .to_string();
            let mut file = File::create(build_dir.path().join(&download_file_name)).await?;
            while let Some(chunk) = download.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
            }
        }

        Ok((build_dir, download_file_name))
    }

    /// Builds the package.
    ///
    /// Returns the output directory.
    async fn build(
        &self,
        build_dir: &TempDir,
        download_file_name: &str,
    ) -> Result<(TempDir, InstallLog)> {
        let output_dir = TempDir::new().context("failed to create output directory")?;
        let mut log = InstallLog::new(self);
        log.new_install = true;

        // Perform build steps, if any.
        if let Some(build) = &self.build {
            let output = Command::new("zsh")
                .arg("-c")
                .arg(format!("set -e\n{build}"))
                .current_dir(build_dir.path())
                .env("MATCHA_SOURCE", download_file_name)
                .env("MATCHA_OUTPUT", output_dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("failed to spawn build command")?
                .wait_with_output()
                .await?;

            log.exit_code = output.status.code().unwrap_or(1);
            log.stdout = String::from_utf8_lossy(&output.stdout).to_string();
            log.stderr = String::from_utf8_lossy(&output.stderr).to_string();
        }

        Ok((output_dir, log))
    }

    /// Installs the package's build outputs to the package directory.
    ///
    /// Returns the package's directory.
    async fn add_to_package_directory(&self, output_dir: &TempDir) -> Result<PathBuf> {
        // Create the package directory.
        let pkg_path = PACKAGE_ROOT
            .get()
            .ok_or(anyhow!("package root is not initialized"))?
            .join(&self.name)
            .join(&self.version);
        create_dir_all(&pkg_path)
            .await
            .context("failed to create package directory")?;

        // Move build outputs to the workspace/package directory.
        rename(output_dir, &pkg_path)
            .await
            .context("failed to move build outputs into package directory")?;

        Ok(pkg_path)
    }

    /// Sets up symlinks from the package directory to the workspace bin directory.
    async fn add_to_workspace(&self, pkg_dir: &Path, workspace: &Workspace) -> Result<()> {
        let pkg_bin_path = pkg_dir.join("bin");
        let workspace_bin_path = workspace.bin_directory()?;
        create_dir_all(workspace_bin_path.clone())
            .await
            .context("failed to create workspace bin directory")?;
        if metadata(&pkg_bin_path).await.is_ok_and(|m| m.is_dir()) {
            let mut pkg_bin_dir_reader = read_dir(&pkg_bin_path).await?;
            while let Some(entry) = pkg_bin_dir_reader.next_entry().await? {
                let target = entry.path();
                let link = workspace_bin_path.join(entry.file_name());
                symlink(&target, &link).await?;
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
        write!(
            f,
            " < {}",
            self.registry
                .as_ref()
                .expect("package not tied to registry")
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::download::MockDownloader;

    use super::*;

    #[test]
    fn test_parse_manifest() -> Result<()> {
        let manifest = r#"
            schema_version = 1
            name = "test"
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

        let manifest: Manifest = manifest.parse()?;

        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.name, "test");
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
        assert_eq!(
            manifest.packages[0].source,
            Some("https://example.invalid/test-package/archive/0.1.0.tar.gz".to_string())
        );
        assert_eq!(
            manifest.packages[0].build,
            Some("cargo build --release".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_download_package_source() -> Result<()> {
        let package = Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            registry: Some("https://example.invalid/registry".to_string()),
            source: Some("https://example.invalid/test-package/archive/0.1.0.tar.gz".to_string()),
            ..Default::default()
        };

        let (build_dir, download_file_name) = package
            .download_source(&MockDownloader::new(vec![]))
            .await?;
        assert!(build_dir.path().exists());
        assert!(build_dir.path().is_dir());
        assert!(download_file_name.ends_with(".tar.gz"));
        Ok(())
    }

    #[tokio::test]
    async fn test_build_package() -> Result<()> {
        let package = Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            registry: Some("https://example.invalid/registry".to_string()),
            source: Some("https://example.invalid/test-source".to_string()),
            build: Some(
                "mkdir $MATCHA_OUTPUT/bin && cp $MATCHA_SOURCE $MATCHA_OUTPUT/bin/".to_string(),
            ),
            ..Default::default()
        };

        let (build_dir, download_file_name) = package
            .download_source(&MockDownloader::new("foo".as_bytes().to_vec()))
            .await?;
        let (output_dir, _log) = package.build(&build_dir, &download_file_name).await?;

        let output_bin_dir = output_dir.path().join("bin");
        assert!(output_bin_dir.exists());
        assert!(output_bin_dir.is_dir());
        assert!(output_bin_dir.join("test-source").exists());
        assert_eq!(
            tokio::fs::read_to_string(output_bin_dir.join("test-source")).await?,
            "foo"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_build_package_without_source() -> Result<()> {
        let package = Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            registry: Some("https://example.invalid/registry".to_string()),
            build: Some("echo hullo > $MATCHA_OUTPUT/output".to_string()),
            ..Default::default()
        };

        let (build_dir, download_file_name) = package
            .download_source(&MockDownloader::new("foo".as_bytes().to_vec()))
            .await?;
        let (output_dir, _log) = package.build(&build_dir, &download_file_name).await?;

        assert!(output_dir.path().exists());
        assert!(output_dir.path().is_dir());
        assert_eq!(
            tokio::fs::read_to_string(output_dir.path().join("output")).await?,
            "hullo\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_build_package_exists_on_first_error() -> Result<()> {
        let package = Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            registry: Some("https://example.invalid/registry".to_string()),
            build: Some("false\ntrue".to_string()),
            ..Default::default()
        };

        let (build_dir, download_file_name) = package
            .download_source(&MockDownloader::new("foo".as_bytes().to_vec()))
            .await?;
        let (_output_dir, log) = package.build(&build_dir, &download_file_name).await?;

        assert!(!log.is_success());
        Ok(())
    }

    #[tokio::test]
    async fn test_add_package_to_package_directory() -> Result<()> {
        let workspace_root = TempDir::new().unwrap();
        crate::WORKSPACE_ROOT
            .set(workspace_root.path().to_owned())
            .unwrap();
        let package_root = TempDir::new().unwrap();
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let package = Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            registry: Some("https://example.invalid/registry".to_string()),
            source: Some("https://example.invalid/test-source".to_string()),
            build: Some(
                "mkdir $MATCHA_OUTPUT/bin && cp $MATCHA_SOURCE $MATCHA_OUTPUT/bin/".to_string(),
            ),
            ..Default::default()
        };

        let (build_dir, download_file_name) = package
            .download_source(&MockDownloader::new("foo".as_bytes().to_vec()))
            .await?;
        let (output_dir, _log) = package.build(&build_dir, &download_file_name).await?;
        package.add_to_package_directory(&output_dir).await?;

        let pkg_path = crate::PACKAGE_ROOT
            .get()
            .unwrap()
            .join(&package.name)
            .join(&package.version);
        assert!(pkg_path.exists());
        assert!(pkg_path.is_dir());
        assert!(pkg_path.join("bin").exists());
        assert!(pkg_path.join("bin").is_dir());
        assert!(pkg_path.join("bin").join("test-source").exists());
        assert_eq!(
            tokio::fs::read_to_string(pkg_path.join("bin").join("test-source")).await?,
            "foo"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_add_installed_package_to_workspace() -> Result<()> {
        let workspace_root = TempDir::new()?;
        crate::WORKSPACE_ROOT
            .set(workspace_root.path().to_owned())
            .unwrap();
        let package_root = TempDir::new()?;
        crate::PACKAGE_ROOT
            .set(package_root.path().to_owned())
            .unwrap();
        let workspace = Workspace::new("test-workspace").await?;
        let package = Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            registry: Some("https://example.invalid/registry".to_string()),
            source: Some("https://example.invalid/test-source".to_string()),
            build: Some(
                "mkdir $MATCHA_OUTPUT/bin && cp $MATCHA_SOURCE $MATCHA_OUTPUT/bin/".to_string(),
            ),
            ..Default::default()
        };

        let (build_dir, download_file_name) = package
            .download_source(&MockDownloader::new("foo".as_bytes().to_vec()))
            .await?;
        let (output_dir, _log) = package.build(&build_dir, &download_file_name).await?;
        let pkg_dir = package.add_to_package_directory(&output_dir).await?;
        package.add_to_workspace(&pkg_dir, &workspace).await?;

        let workspace_bin_path = workspace.bin_directory()?;
        assert!(workspace_bin_path.exists());
        assert!(workspace_bin_path.is_dir());
        assert!(workspace_bin_path.join("test-source").exists());
        assert!(workspace_bin_path.join("test-source").is_file());
        assert_eq!(
            tokio::fs::read_to_string(workspace_bin_path.join("test-source")).await?,
            "foo"
        );

        Ok(())
    }
}

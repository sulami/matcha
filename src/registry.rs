use std::{fmt::Display, future::Future, path::PathBuf, str::FromStr, time::Duration};

use color_eyre::eyre::{anyhow, Context, Result};
use sqlx::{sqlite::SqliteRow, FromRow, Row};
use time::OffsetDateTime;
use tokio::fs::read_to_string;
use tracing::instrument;

use crate::{
    download::download_file, manifest::Manifest, package::KnownPackage, state::State,
    util::is_file_system_safe,
};

#[cfg(test)]
use crate::manifest::Package;

/// How often to update registries.
const UPDATE_AFTER: Duration = Duration::from_secs(60 * 24);

/// A registry is a place that has manifests.
#[derive(Debug)]
pub struct Registry {
    /// The name of the registry.
    ///
    /// Can be blank if we have never fetched it.
    pub name: Option<String>,
    /// The URI of the registry.
    pub uri: Uri,
    /// The last time this registry was fetched.
    pub last_fetched: Option<OffsetDateTime>,
}

/// A registry URI.
#[derive(Debug, PartialEq, Eq)]
pub enum Uri {
    /// A local file path.
    File(PathBuf),
    /// An HTTP URI.
    Http(String),
    /// An HTTPS URI.
    Https(String),
}

impl Registry {
    /// Creates a new registry.
    pub fn new(uri: &str) -> Self {
        Self {
            name: None,
            uri: uri.into(),
            last_fetched: None,
        }
    }

    /// Do the initial fetch of the registry and write it to the database.
    #[instrument(skip(state, fetcher))]
    pub async fn initialize(&mut self, state: &State, fetcher: &impl Fetcher) -> Result<()> {
        let manifest = self.download(fetcher).await?;

        self.name = Some(manifest.name.clone());
        state.add_registry(self).await?;

        Ok(())
    }

    /// Returns if the registry is initialized, and can be written to the database.
    pub fn is_initialized(&self) -> bool {
        self.name.is_some()
    }

    /// Fetches the manifest from the registry and stores updates in the database.
    #[instrument(skip(state, fetcher))]
    pub async fn fetch(&mut self, state: &State, fetcher: &impl Fetcher) -> Result<()> {
        let manifest = self.download(fetcher).await?;

        // TODO: Keep and compare a manifest hash to avoid unnecessary updates.

        if let Some(pkg) = manifest
            .packages
            .iter()
            .find(|p| !is_file_system_safe(&p.name) || !is_file_system_safe(&p.version))
        {
            return Err(anyhow!("invalid package name or version: {}", pkg));
        }

        // Check if any packages collide with another registry's ones.
        let collisions = {
            let mut collisions = Vec::new();
            for pkg in &manifest.packages {
                if let Some(other) = state
                    .get_known_package(&KnownPackage {
                        name: pkg.name.clone(),
                        version: pkg.version.clone(),
                    })
                    .await
                    .wrap_err("failed to check for pre-existing known package")?
                {
                    if other.registry.as_ref().expect("orphaned package found")
                        != &self.uri.to_string()
                    {
                        collisions.push((pkg, other));
                    }
                }
            }
            collisions
        };
        if !collisions.is_empty() {
            let mut msg = String::new();
            for (pkg, other) in collisions {
                msg.push_str(&format!(
                    "{}'s package {} collides with {}'s",
                    pkg.registry.as_ref().unwrap(),
                    pkg.name,
                    other.registry.unwrap(),
                ));
            }
            return Err(anyhow!(msg));
        }

        // Remove packages that are no longer in the manifest.
        let know_packages = state.known_packages_for_registry(self).await?;
        for pkg in &know_packages {
            if !manifest.packages.contains(pkg) {
                state
                    .remove_known_package(&KnownPackage {
                        name: pkg.name.clone(),
                        version: pkg.version.clone(),
                    })
                    .await?;
            }
        }

        // Add new packages.
        state
            .add_known_packages(&manifest.packages)
            .await
            .wrap_err("failed to add new known packages")?;

        // Update name if changed.
        self.name = Some(manifest.name.clone());
        self.last_fetched = Some(OffsetDateTime::now_utc());
        state
            .update_registry(self)
            .await
            .wrap_err("failed to update registry in database")?;

        Ok(())
    }

    /// Fetches the manifest from the registry.
    #[instrument(skip(fetcher))]
    async fn download(&self, fetcher: &impl Fetcher) -> Result<Manifest> {
        let s = fetcher.fetch(self).await?;
        let mut manifest: Manifest = s.parse().wrap_err("failed to parse manifest")?;
        manifest.set_registry_uri(&self.uri.to_string());
        Ok(manifest)
    }

    /// Returns if the registry should be fetched.
    #[instrument]
    pub fn should_update(&self) -> bool {
        if let Uri::File(_) = self.uri {
            return true;
        }
        let now = OffsetDateTime::now_utc();
        let Some(last_fetched) = self.last_fetched else {
            return true;
        };
        let elapsed = now - last_fetched;
        elapsed >= UPDATE_AFTER
    }
}

impl Display for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.uri)?;
        if let Some(name) = &self.name {
            write!(f, " ({})", name)?;
        }
        Ok(())
    }
}

impl Display for Uri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Uri::File(path) => write!(f, "{}", path.display()),
            Uri::Http(uri) => write!(f, "{}", uri),
            Uri::Https(uri) => write!(f, "{}", uri),
        }
    }
}

impl FromRow<'_, SqliteRow> for Registry {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        let name: String = row.try_get("name")?;
        let uri: String = row.try_get("uri")?;
        let last_fetched: Option<OffsetDateTime> = row.try_get("last_fetched")?;
        Ok(Self {
            name: Some(name),
            uri: uri.into(),
            last_fetched,
        })
    }
}

impl From<String> for Uri {
    fn from(s: String) -> Self {
        s.as_str().into()
    }
}

impl From<&str> for Uri {
    fn from(s: &str) -> Self {
        if s.starts_with("http://") {
            Self::Http(s.into())
        } else if s.starts_with("https://") {
            Self::Https(s.into())
        } else {
            let path = PathBuf::from(s);
            // Resolve to absolute path.
            let path = if path.is_relative() {
                std::env::current_dir()
                    .expect("failed to get current working directory")
                    .join(path)
                    .to_str()
                    .unwrap()
                    .into()
            } else {
                path.to_str().unwrap().into()
            };
            Self::File(path)
        }
    }
}

impl FromStr for Uri {
    type Err = color_eyre::eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
    }
}

#[cfg(test)]
impl Default for Registry {
    fn default() -> Self {
        Self {
            name: Some("test".into()),
            uri: "https://example.invalid/test".into(),
            last_fetched: None,
        }
    }
}

/// A fetcher fetches a manifest from a registry.
///
/// This trait exists so that we can mock out fetching for tests.
pub trait Fetcher: Send + Sync + Clone {
    /// Fetches the manifest string from the registry.
    fn fetch(&self, reg: &Registry) -> impl Future<Output = Result<String>> + Send;
}

/// The default fetcher, which fetches from the filesystem or HTTP.
#[derive(Debug, Copy, Clone, Default)]
pub struct DefaultFetcher;

impl Fetcher for DefaultFetcher {
    #[instrument]
    async fn fetch(&self, reg: &Registry) -> Result<String> {
        let s = match &reg.uri {
            Uri::File(path) => read_to_string(path)
                .await
                .wrap_err("failed to read manifest at {path}")?,
            Uri::Http(uri) | Uri::Https(uri) => {
                let bytes = download_file(uri)
                    .await
                    .wrap_err("failed to fetch manifest from {uri}")?;
                String::from_utf8(bytes).wrap_err("failed to parse downloaded manifest as utf-8")?
            }
        };
        Ok(s)
    }
}

#[cfg(test)]
/// A mock fetcher, which returns a pre-defined manifest.
#[derive(Debug, Clone)]
pub struct MockFetcher {
    pub manifest: String,
}

#[cfg(test)]
impl MockFetcher {
    /// Creates a new mock fetcher that returns a manifest with the given packages.
    pub fn with_packages(pkgs: &[Package]) -> Self {
        let manifest = Manifest {
            schema_version: 1,
            name: "test".into(),
            packages: pkgs.into(),
            ..Default::default()
        };
        Self {
            manifest: toml::to_string_pretty(&manifest).unwrap(),
        }
    }
}

#[cfg(test)]
impl Default for MockFetcher {
    fn default() -> Self {
        Self {
            manifest: r#"
                schema_version = 1
                name = "test"
                uri = "https://example.invalid/registry"
                description = "A test manifest"

                [[packages]]
                name = "test-package"
                version = "0.1.0"

                [[packages]]
                name = "test-package"
                version = "0.1.1"

                [[packages]]
                name = "another-package"
                version = "0.2.0"

                [[packages]]
                name = "failing-build"
                version = "0.1.0"
                build = "exit 1"
            "#
            .into(),
        }
    }
}

#[cfg(test)]
impl Fetcher for MockFetcher {
    async fn fetch(&self, _reg: &Registry) -> Result<String> {
        Ok(self.manifest.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_from_str() -> Result<()> {
        assert_eq!(
            Uri::from_str("http://example.invalid")?,
            Uri::Http("http://example.invalid".into())
        );
        assert_eq!(
            Uri::from_str("https://example.invalid")?,
            Uri::Https("https://example.invalid".into())
        );
        // Local paths should be absolute.
        let pwd = std::env::current_dir()?;
        assert_eq!(
            Uri::from_str("example")?,
            Uri::File(pwd.join("example").to_str().unwrap().into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_is_initialized() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        assert!(!registry.is_initialized());
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();
        assert!(registry.is_initialized());
    }

    #[tokio::test]
    async fn test_should_update() {
        let mut registry = Registry::new("https://example.invalid/registry");
        assert!(registry.should_update());
        registry.last_fetched = Some(OffsetDateTime::now_utc());
        assert!(!registry.should_update());
    }

    #[tokio::test]
    async fn test_should_update_always_updates_local_files() {
        let mut registry = Registry::new("example");
        assert!(registry.should_update());
        registry.last_fetched = Some(OffsetDateTime::now_utc());
        assert!(registry.should_update());
    }

    #[tokio::test]
    async fn test_update_registry() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();
        registry
            .fetch(&state, &MockFetcher::default())
            .await
            .unwrap();
        assert_eq!(registry.name, Some("test".into()));
        assert!(registry.last_fetched.is_some());
    }

    #[tokio::test]
    async fn test_update_registry_refuses_unsafe_package_names() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();
        let unsafe_package = Package {
            name: "test/package".into(),
            version: "0.1.0".into(),
            ..Default::default()
        };
        assert!(registry
            .fetch(&state, &MockFetcher::with_packages(&[unsafe_package]))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_update_package_refuses_overwriting_other_registrys_package() -> Result<()> {
        let state = State::load(":memory:").await?;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&state, &MockFetcher::default()).await?;
        registry.fetch(&state, &MockFetcher::default()).await?;
        let mut second_registry = Registry::new("https://example.invalid/second-registry");
        second_registry
            .initialize(&state, &MockFetcher::default())
            .await?;
        let res = second_registry.fetch(&state, &MockFetcher::default()).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("collides with"));
        Ok(())
    }
}

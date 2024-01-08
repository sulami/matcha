use std::{fmt::Display, future::Future, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result};
use sqlx::{sqlite::SqliteRow, FromRow, Row};
use time::OffsetDateTime;
use tokio::fs::read_to_string;

use crate::{download::download_file, manifest::Manifest, state::State};

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

    /// Do the initial fetch of the registry and prep it for writing to the database.
    pub async fn initialize(&mut self, fetcher: &impl Fetcher) -> Result<()> {
        let manifest = self.fetch(fetcher).await?;

        self.name = Some(manifest.name.clone());

        Ok(())
    }

    /// Returns if the registry is initialized, and can be written to the database.
    pub fn is_initialized(&self) -> bool {
        self.name.is_some()
    }

    /// Fetches the manifest from the registry and stores updates in the database.
    pub async fn update(&mut self, state: &State, fetcher: &impl Fetcher) -> Result<()> {
        let manifest = self.fetch(fetcher).await?;

        // TODO: Keep and compare a manifest hash to avoid unnecessary updates.

        state.add_known_packages(&manifest.packages).await?;

        // Update name if changed.
        self.name = Some(manifest.name.clone());
        self.last_fetched = Some(OffsetDateTime::now_utc());
        state.update_registry(self).await?;

        Ok(())
    }

    /// Fetches the manifest from the registry.
    async fn fetch(&self, fetcher: &impl Fetcher) -> Result<Manifest> {
        let s = fetcher.fetch(self).await?;
        let manifest: Manifest = s.parse().context("failed to parse manifest")?;
        Ok(manifest)
    }

    /// Returns if the registry should be fetched.
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
        write!(
            f,
            "{} ({})",
            self.name.as_deref().unwrap_or("<unknown>"),
            self.uri
        )
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
            Self::File(s.into())
        }
    }
}

impl FromStr for Uri {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
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
    async fn fetch(&self, reg: &Registry) -> Result<String> {
        let s = match &reg.uri {
            Uri::File(path) => read_to_string(path)
                .await
                .context("failed to read manifest at {path}")?,
            Uri::Http(uri) | Uri::Https(uri) => {
                let bytes = download_file(uri, None)
                    .await
                    .context("failed to fetch manifest from {uri}")?;
                String::from_utf8(bytes).context("failed to parse downloaded manifest as utf-8")?
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
impl Default for MockFetcher {
    fn default() -> Self {
        Self {
            manifest: r#"
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
    fn test_uri_from_str() {
        assert_eq!(
            Uri::from_str("http://example.invalid").unwrap(),
            Uri::Http("http://example.invalid".into())
        );
        assert_eq!(
            Uri::from_str("https://example.invalid").unwrap(),
            Uri::Https("https://example.invalid".into())
        );
        assert_eq!(
            Uri::from_str("example").unwrap(),
            Uri::File("example".into())
        );
    }

    #[tokio::test]
    async fn test_is_initialized() {
        let mut registry = Registry::new("https://example.invalid");
        assert!(!registry.is_initialized());
        registry.initialize(&MockFetcher::default()).await.unwrap();
        assert!(registry.is_initialized());
    }

    #[tokio::test]
    async fn test_should_update() {
        let mut registry = Registry::new("https://example.invalid");
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
        let mut registry = Registry::new("https://example.invalid");
        registry.initialize(&MockFetcher::default()).await.unwrap();
        state.add_registry(&registry).await.unwrap();
        registry
            .update(&state, &MockFetcher::default())
            .await
            .unwrap();
        assert_eq!(registry.name, Some("test".into()));
        assert!(registry.last_fetched.is_some());
    }
}

use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use sqlx::{sqlite::SqliteRow, FromRow, Row};
use time::OffsetDateTime;
use tokio::sync::watch;

use crate::download::download_file;
use crate::manifest::Manifest;

/// A registry is a place that has manifests.
#[derive(Debug)]
pub struct Registry {
    /// The name of the registry.
    pub name: String,
    /// The URI of the registry.
    pub uri: Uri,
    /// The last time this registry was fetched.
    pub last_fetched: Option<OffsetDateTime>,
}

/// A registry URI.
#[derive(Debug)]
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
    pub fn new(name: &str, uri: &str) -> Self {
        Self {
            name: name.into(),
            uri: uri.into(),
            last_fetched: None,
        }
    }

    /// Fetches the manifest from the registry.
    pub async fn fetch(&mut self) -> Result<Manifest> {
        let s = match &self.uri {
            Uri::File(path) => tokio::fs::read_to_string(path)
                .await
                .context("failed to read manifest at {path}")?,
            Uri::Http(uri) | Uri::Https(uri) => {
                let (tx, rx) = watch::channel(0);
                let bytes = download_file(uri, tx)
                    .await
                    .context("failed to fetch manifest from {uri}")?;
                String::from_utf8(bytes).context("failed to parse downloaded manifest as utf-8")?
            }
        };
        let manifest = s.parse().context("failed to parse manifest")?;
        self.last_fetched = Some(OffsetDateTime::now_local()?);
        Ok(manifest)
    }
}

impl Display for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.uri)
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
        Ok(Self::new(&name, &uri))
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

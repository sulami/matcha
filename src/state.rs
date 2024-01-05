use std::{
    io::{stderr, Write},
    path::Path,
    str::FromStr,
};

use anyhow::{anyhow, Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use tokio::fs::create_dir_all;

use crate::package::Package;

/// The internal state of the application, backed by a SQLite database.
#[derive(Clone)]
pub struct State {
    /// The database connection pool.
    db: SqlitePool,
}

impl State {
    /// Loads the internal state database from the given path.
    pub async fn load(path: &str) -> Result<Self> {
        let path = &shellexpand::tilde(path).to_string();
        let db = if !Path::new(path).exists() {
            Self::init(path)
                .await
                .context("failed to initialize database")?
        } else {
            Self::connect_db(path)
                .await
                .context("failed to connect to database")?
        };

        let schema_version: String =
            sqlx::query_scalar("SELECT value FROM meta WHERE key = 'schema_version'")
                .fetch_one(&db)
                .await
                .context("failed to fetch schema version from database")?;
        if schema_version
            .parse::<i64>()
            .context("failed to parse database schema version")?
            > 1
        {
            return Err(anyhow!(
                "unsupported database schema version {}",
                schema_version
            ));
        }

        Ok(Self { db })
    }

    /// Initializes the internal state database at the given path.
    async fn init(path: &str) -> Result<SqlitePool> {
        writeln!(
            stderr(),
            "No state database found, creating a new one at {}",
            path
        )?;

        // Create the directory if it doesn't exist.
        let dir = Path::new(path).parent().unwrap();
        if !dir.exists() {
            create_dir_all(dir)
                .await
                .context("failed to create state directory")?;
        }

        // Create the database schema.
        let db = Self::connect_db(path)
            .await
            .context("failed to create new database")?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (key)
            );
            INSERT INTO meta (key, value) VALUES ('schema_version', '1');

            CREATE TABLE IF NOT EXISTS installed_packages (
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                PRIMARY KEY (name, version)
            );

            CREATE TABLE IF NOT EXISTS registries (
                uri TEXT NOT NULL,
                PRIMARY KEY (uri)
            );
            "#,
        )
        .execute(&db)
        .await
        .context("failed to initialize database schema")?;
        Ok(db)
    }

    /// Connects to the database at the given path, creating it if it doesn't exist.
    async fn connect_db(path: &str) -> Result<SqlitePool> {
        let db =
            SqlitePool::connect_with(SqliteConnectOptions::from_str(path)?.create_if_missing(true))
                .await?;

        Ok(db)
    }

    /// Returns all installed packages.
    pub async fn installed_packages(&self) -> Result<Vec<Package>> {
        let packages = sqlx::query_as::<_, Package>("SELECT name, version FROM installed_packages")
            .fetch_all(&self.db)
            .await
            .context("failed to fetch installed packages from database")?;
        Ok(packages)
    }

    /// Adds a package to the internal state.
    pub async fn add_installed_package(&self, pkg: &Package) -> Result<()> {
        if !pkg.is_fully_qualified() {
            return Err(anyhow!("package {} is not fully qualified", pkg));
        }
        sqlx::query("INSERT INTO installed_packages (name, version) VALUES (?, ?)")
            .bind(&pkg.name)
            .bind(&pkg.version)
            .execute(&self.db)
            .await
            .context("failed to insert installed package into database")?;
        Ok(())
    }

    /// Removes a package from the internal state.
    pub async fn remove_installed_package(&self, pkg: &Package) -> Result<()> {
        if !pkg.is_fully_qualified() {
            return Err(anyhow!("package {} is not fully qualified", pkg));
        }
        sqlx::query("DELETE FROM installed_packages WHERE name = ? AND version = ?")
            .bind(&pkg.name)
            .bind(&pkg.version)
            .execute(&self.db)
            .await
            .context("failed to remove installed package from database")?;
        Ok(())
    }

    /// Resolves a package to its latest installed version.
    ///
    /// Returns an error if the package is either not installed,
    /// or if multiple versions of the package are installed.
    pub async fn resolve_installed_package_version(&self, pkg: &mut Package) -> Result<()> {
        if !pkg.is_fully_qualified() {
            let installed_versions = self.installed_package_versions(pkg).await?;
            if installed_versions.is_empty() {
                return Err(anyhow!("package {} is not installed", pkg));
            }
            if installed_versions.len() > 1 {
                return Err(anyhow!(
                    "multiple versions of package {} are installed: {}",
                    pkg.name,
                    installed_versions.join(", ")
                ));
            }
            pkg.version = Some(installed_versions.first().unwrap().clone());
        } else if !self.is_package_installed(pkg).await? {
            return Err(anyhow!("package {} is not installed", pkg));
        }

        Ok(())
    }

    /// Returns whether a package is installed or not.
    ///
    /// If the package version is not specified, this will return `true`
    /// if any version of the package is installed.
    pub async fn is_package_installed(&self, pkg: &Package) -> Result<bool> {
        if pkg.is_fully_qualified() {
            // Find this specific version.
            Ok(self
                .installed_package_versions(pkg)
                .await?
                .iter()
                .any(|v| v == pkg.version.as_ref().unwrap()))
        } else {
            // Find any version.
            Ok(!self.installed_package_versions(pkg).await?.is_empty())
        }
    }

    /// Returns all installed versions of a package, ordered newest to oldest.
    async fn installed_package_versions(&self, pkg: &Package) -> Result<Vec<String>> {
        let versions = sqlx::query_scalar(
            "SELECT version FROM installed_packages WHERE name = ? ORDER BY version DESC",
        )
        .bind(&pkg.name)
        .fetch_all(&self.db)
        .await
        .context("failed to fetch installed package versions from database")?;
        Ok(versions)
    }

    /// Adds a registry to the internal state.
    pub async fn add_registry(&self, uri: &str) -> Result<()> {
        if self.registry_exists(uri).await? {
            return Err(anyhow!("registry {} already exists", uri));
        }
        sqlx::query("INSERT INTO registries (uri) VALUES (?)")
            .bind(uri)
            .execute(&self.db)
            .await
            .context("failed to insert registry into database")?;
        Ok(())
    }

    /// Removes a registry from the internal state.
    pub async fn remove_registry(&self, uri: &str) -> Result<()> {
        if !self.registry_exists(uri).await? {
            return Err(anyhow!("registry {} does not exist", uri));
        }
        sqlx::query("DELETE FROM registries WHERE uri = ?")
            .bind(uri)
            .execute(&self.db)
            .await
            .context("failed to remove registry from database")?;
        Ok(())
    }

    /// Returns all registries.
    pub async fn registries(&self) -> Result<Vec<String>> {
        let registries = sqlx::query_scalar("SELECT uri FROM registries")
            .fetch_all(&self.db)
            .await
            .context("failed to fetch registries from database")?;
        Ok(registries)
    }

    /// Returns true if the registry exists.
    async fn registry_exists(&self, uri: &str) -> Result<bool> {
        let exists = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM registries WHERE uri = ?)")
            .bind(uri)
            .fetch_one(&self.db)
            .await
            .context("failed to check if registry exists in database")?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_package_add_list_remove() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        let packages = state.installed_packages().await.unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "foo");
        assert_eq!(packages[0].version, Some("1.0.0".to_string()));
        state
            .remove_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        let packages = state.installed_packages().await.unwrap();
        assert!(packages.is_empty());
    }

    #[tokio::test]
    async fn test_add_package_refuses_same_version_twice() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        assert!(state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_remove_package_refuses_unqualified_version() {
        let state = State::load(":memory:").await.unwrap();
        assert!(state
            .remove_installed_package(&Package {
                name: "foo".to_string(),
                version: None,
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_is_package_installed() {
        let state = State::load(":memory:").await.unwrap();
        assert!(!state
            .is_package_installed(&Package {
                name: "foo".to_string(),
                version: None,
            })
            .await
            .unwrap());
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        assert!(state
            .is_package_installed(&Package {
                name: "foo".to_string(),
                version: None,
            })
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_resolve_installed_package_version() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: None,
        };
        state
            .resolve_installed_package_version(&mut pkg)
            .await
            .unwrap();
        assert_eq!(pkg.version, Some("1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_installed_package_version_fails_if_not_installed() {
        let state = State::load(":memory:").await.unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: None,
        };
        assert!(state
            .resolve_installed_package_version(&mut pkg)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_resolve_installed_package_version_fails_if_this_version_is_not_installed() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: Some("2.0.0".to_string()),
        };
        assert!(state
            .resolve_installed_package_version(&mut pkg)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_resolve_installed_package_version_fails_if_multiple_installed() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("1.0.0".to_string()),
            })
            .await
            .unwrap();
        state
            .add_installed_package(&Package {
                name: "foo".to_string(),
                version: Some("2.0.0".to_string()),
            })
            .await
            .unwrap();
        let mut pkg = Package {
            name: "foo".to_string(),
            version: None,
        };
        assert!(state
            .resolve_installed_package_version(&mut pkg)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_registry_add_list_remove() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_registry("https://example.invalid/registry")
            .await
            .unwrap();
        let registries = state.registries().await.unwrap();
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0], "https://example.invalid/registry");
        state
            .remove_registry("https://example.invalid/registry")
            .await
            .unwrap();
        let registries = state.registries().await.unwrap();
        assert!(registries.is_empty());
    }

    #[tokio::test]
    async fn test_add_registry_refuses_same_uri_twice() {
        let state = State::load(":memory:").await.unwrap();
        state
            .add_registry("https://example.invalid/registry")
            .await
            .unwrap();
        assert!(state
            .add_registry("https://example.invalid/registry")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_remove_registry_refuses_nonexistent_uri() {
        let state = State::load(":memory:").await.unwrap();
        assert!(state
            .remove_registry("https://example.invalid/registry")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_registry_exists() {
        let state = State::load(":memory:").await.unwrap();
        assert!(!state
            .registry_exists("https://example.invalid/registry")
            .await
            .unwrap());
        state
            .add_registry("https://example.invalid/registry")
            .await
            .unwrap();
        assert!(state
            .registry_exists("https://example.invalid/registry")
            .await
            .unwrap());
    }
}

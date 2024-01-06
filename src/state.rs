use std::{
    io::{stderr, Write},
    path::Path,
    str::FromStr,
};

use anyhow::{anyhow, Context, Result};
use sqlx::{
    migrate,
    sqlite::{SqliteConnectOptions, SqlitePool},
};
use tokio::fs::create_dir_all;

use crate::{manifest::Package as ManifestPackage, package::Package, registry::Registry};

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
        migrate!("./migrations")
            .run(&db)
            .await
            .context("failed to initialize database")?;
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
    pub async fn installed_package_versions(&self, pkg: &Package) -> Result<Vec<String>> {
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
    pub async fn add_registry(&self, reg: &Registry) -> Result<()> {
        if !reg.is_initialized() {
            return Err(anyhow!("registry {} is not initialized", &reg.uri));
        }
        if self
            .registry_exists_by_name(reg.name.as_ref().unwrap())
            .await?
        {
            return Err(anyhow!(
                "registry {} already exists",
                reg.name.as_ref().unwrap()
            ));
        }
        sqlx::query("INSERT INTO registries (name, uri) VALUES (?, ?)")
            .bind(reg.name.as_ref().unwrap())
            .bind(reg.uri.to_string())
            .execute(&self.db)
            .await
            .context("failed to insert registry into database")?;
        Ok(())
    }

    /// Removes a registry from the internal state.
    pub async fn remove_registry(&self, name: &str) -> Result<()> {
        if !self.registry_exists_by_name(name).await? {
            return Err(anyhow!("registry {} does not exist", name));
        }
        sqlx::query("DELETE FROM registries WHERE name = ?")
            .bind(name)
            .execute(&self.db)
            .await
            .context("failed to remove registry from database")?;
        Ok(())
    }

    /// Returns all registries.
    pub async fn registries(&self) -> Result<Vec<Registry>> {
        let registries =
            sqlx::query_as::<_, Registry>("SELECT name, uri, last_fetched FROM registries")
                .fetch_all(&self.db)
                .await
                .context("failed to fetch registries from database")?;
        Ok(registries)
    }

    /// Returns true if a registry with this name exists.
    pub async fn registry_exists_by_name(&self, name: &str) -> Result<bool> {
        let exists = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM registries WHERE name = ?)")
            .bind(name)
            .fetch_one(&self.db)
            .await
            .context("failed to check if registry exists in database")?;
        Ok(exists)
    }

    /// Returns true if a registry with this URI exists.
    pub async fn registry_exists_by_uri(&self, uri: &str) -> Result<bool> {
        let exists = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM registries WHERE uri = ?)")
            .bind(uri)
            .fetch_one(&self.db)
            .await
            .context("failed to check if registry exists in database")?;
        Ok(exists)
    }

    /// Updates the database record of a registry with a new name and last_fetched.
    pub async fn update_registry(&self, reg: &Registry) -> Result<()> {
        if !self.registry_exists_by_uri(&reg.uri.to_string()).await? {
            return Err(anyhow!("registry {} does not exist", &reg.uri));
        }
        sqlx::query("UPDATE registries SET name = ?, last_fetched = ? WHERE uri = ?")
            .bind(&reg.name)
            .bind(reg.last_fetched)
            .bind(&reg.uri.to_string())
            .execute(&self.db)
            .await
            .context("failed to update registry last_fetched in database")?;
        Ok(())
    }

    /// Adds known packages to the database.
    pub async fn add_known_packages(&self, pkgs: &[ManifestPackage]) -> Result<()> {
        for pkg in pkgs {
            sqlx::query(
                "INSERT INTO known_packages
                    (name, version, description, homepage, license, registry)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT (name, version)
                    DO UPDATE
                    SET description = $3, homepage = $4, license = $5, registry = $6
                    WHERE name = $1 AND version = $2",
            )
            .bind(&pkg.name)
            .bind(&pkg.version)
            .bind(&pkg.description)
            .bind(&pkg.homepage)
            .bind(&pkg.license)
            .bind(&pkg.registry)
            .execute(&self.db)
            .await
            .context("failed to insert known package into database")?;
        }
        Ok(())
    }

    /// Searches known packages for a query.
    pub async fn search_known_packages(&self, query: &str) -> Result<Vec<ManifestPackage>> {
        let query = format!("%{}%", query);
        let pkgs = sqlx::query_as::<_, ManifestPackage>("SELECT name, version, description, homepage, license, registry FROM known_packages WHERE name LIKE ? OR description LIKE ? OR homepage LIKE ? ORDER BY name ASC, version DESC")
            .bind(&query)
            .bind(&query)
            .bind(&query)
            .fetch_all(&self.db)
            .await
            .context("failed to fetch known packages from database")?;
        Ok(pkgs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::OffsetDateTime;

    use crate::registry::MockFetcher;

    /// Convenience function to setup the default test state.
    async fn setup_state_with_registry() -> Result<State> {
        let state = State::load(":memory:").await?;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&MockFetcher::default()).await?;
        state.add_registry(&registry).await?;
        Ok(state)
    }

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
    async fn test_registry_add_list_remove() {
        let state = setup_state_with_registry().await.unwrap();

        let registries = state.registries().await.unwrap();
        assert_eq!(registries.len(), 1);
        assert_eq!(
            registries[0].uri.to_string(),
            "https://example.invalid/registry"
        );
        state.remove_registry("test").await.unwrap();
        let registries = state.registries().await.unwrap();
        assert!(registries.is_empty());
    }

    #[tokio::test]
    async fn test_add_registry_refuses_same_name_twice() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&MockFetcher::default()).await.unwrap();
        state.add_registry(&registry).await.unwrap();
        assert!(state.add_registry(&registry).await.is_err());
    }

    #[tokio::test]
    async fn test_remove_registry_refuses_nonexistent_name() {
        let state = State::load(":memory:").await.unwrap();
        assert!(state.remove_registry("test").await.is_err());
    }

    #[tokio::test]
    async fn test_registry_exists() {
        let state = setup_state_with_registry().await.unwrap();
        assert!(state.registry_exists_by_name("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_update_registry() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&MockFetcher::default()).await.unwrap();
        state.add_registry(&registry).await.unwrap();

        let new_name = "foo".to_string();
        let last_fetched = OffsetDateTime::now_utc();

        registry.name = Some(new_name.clone());
        registry.last_fetched = Some(last_fetched);
        state.update_registry(&registry).await.unwrap();

        let registries = state.registries().await.unwrap();
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].name, Some(new_name));
        assert_eq!(registries[0].last_fetched, Some(last_fetched));
    }

    #[tokio::test]
    async fn test_search_known_packages() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![
            ManifestPackage {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/foo".to_string()),
                license: Some("MIT".to_string()),
                registry: "test".to_string(),
            },
            ManifestPackage {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/bar".to_string()),
                license: Some("MIT".to_string()),
                registry: "test".to_string(),
            },
            ManifestPackage {
                name: "baz".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/baz".to_string()),
                license: Some("MIT".to_string()),
                registry: "test".to_string(),
            },
        ];
        state.add_known_packages(&pkgs).await.unwrap();
        let results = state.search_known_packages("foo").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "foo");
        assert_eq!(results[0].version, "1.0.0");
        assert_eq!(results[0].description, Some("A test package".to_string()));
        assert_eq!(
            results[0].homepage,
            Some("https://example.invalid/foo".to_string())
        );
        assert_eq!(results[0].license, Some("MIT".to_string()));
        assert_eq!(results[0].registry, "test");
    }

    #[tokio::test]
    async fn test_add_known_packages_updates_existing() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![ManifestPackage {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            description: Some("A test package".to_string()),
            homepage: Some("https://example.invalid/foo".to_string()),
            license: Some("BSD".to_string()),
            registry: "test".to_string(),
        }];
        state.add_known_packages(&pkgs).await.unwrap();
        let results = state.search_known_packages("foo").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "test-package");
        assert_eq!(results[0].version, "0.1.0");
        assert_eq!(results[0].description, Some("A test package".to_string()));
        assert_eq!(
            results[0].homepage,
            Some("https://example.invalid/foo".to_string())
        );
        assert_eq!(results[0].license, Some("BSD".to_string()));
        assert_eq!(results[0].registry, "test");
    }
}

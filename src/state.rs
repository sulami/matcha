use std::{path::Path, str::FromStr};

use anyhow::{anyhow, Context, Result};
use sqlx::{
    migrate,
    sqlite::{Sqlite, SqliteConnectOptions, SqlitePool},
};
use tokio::fs::create_dir_all;

use crate::{
    manifest::Package,
    package::{InstalledPackage, KnownPackageSpec, WorkspacePackageSpec},
    registry::Registry,
    workspace::Workspace,
};

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
        eprintln!("No state database found, creating a new one at {}", path);

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

    /// Begins a transaction.
    #[allow(dead_code)]
    pub async fn begin_transaction(&self) -> Result<sqlx::Transaction<'_, Sqlite>> {
        Ok(self.db.begin().await?)
    }

    /// Commits a transaction.
    #[allow(dead_code)]
    pub async fn commit_transaction(&self, tx: sqlx::Transaction<'_, Sqlite>) -> Result<()> {
        tx.commit().await?;
        Ok(())
    }

    /// Rolls back a transaction.
    #[allow(dead_code)]
    pub async fn rollback_transaction(&self, tx: sqlx::Transaction<'_, Sqlite>) -> Result<()> {
        tx.rollback().await?;
        Ok(())
    }

    /// Returns all installed packages.
    pub async fn workspace_packages(
        &self,
        workspace: &Workspace,
    ) -> Result<Vec<WorkspacePackageSpec>> {
        let packages = sqlx::query_as("SELECT * FROM workspace_packages WHERE workspace = $1")
            .bind(&workspace.name)
            .fetch_all(&self.db)
            .await
            .context("failed to fetch workspace packages from database")?;
        Ok(packages)
    }

    /// Adds an installed package to the internal state.
    pub async fn add_installed_package(&self, pkg: &KnownPackageSpec) -> Result<()> {
        sqlx::query("INSERT INTO installed_packages (name, version) VALUES ($1, $2)")
            .bind(&pkg.name)
            .bind(&pkg.version)
            .execute(&self.db)
            .await
            .context("failed to insert installed package into database")?;
        Ok(())
    }

    pub async fn get_installed_package(
        &self,
        pkg: &KnownPackageSpec,
    ) -> Result<Option<InstalledPackage>> {
        let result =
            sqlx::query_as("SELECT * FROM installed_packages WHERE name = $1 AND version = $2")
                .bind(&pkg.name)
                .bind(&pkg.version)
                .fetch_optional(&self.db)
                .await
                .context("failed to fetch installed package from database")?;
        Ok(result)
    }

    /// Adds a workspace package to the internal state.
    pub async fn add_workspace_package(
        &self,
        pkg: &KnownPackageSpec,
        workspace: &Workspace,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO workspace_packages (name, version, requested_version, workspace) VALUES ($1, $2, $3, $4)",
        )
        .bind(&pkg.name)
        .bind(&pkg.version)
        .bind(&pkg.requested_version)
        .bind(&workspace.name)
        .execute(&self.db)
        .await
        .context("failed to insert workspace package into database")?;
        Ok(())
    }

    /// Returns all installed packages that are not tied to a workspace.
    pub async fn unused_installed_packages(&self) -> Result<Vec<InstalledPackage>> {
        let packages = sqlx::query_as(
            "SELECT * FROM installed_packages WHERE (name, version) NOT IN
               (SELECT name, version FROM workspace_packages)",
        )
        .fetch_all(&self.db)
        .await
        .context("failed to fetch unused installed packages from database")?;
        Ok(packages)
    }

    /// Removes an installed package from the internal state.
    pub async fn remove_installed_package(&self, pkg: &InstalledPackage) -> Result<()> {
        sqlx::query("DELETE FROM installed_packages WHERE name = $1 AND version = $2")
            .bind(&pkg.name)
            .bind(&pkg.version)
            .execute(&self.db)
            .await
            .context("failed to remove installed package from database")?;
        Ok(())
    }

    /// Removes a workspace package from the internal state.
    pub async fn remove_workspace_package(
        &self,
        pkg: &WorkspacePackageSpec,
        workspace: &Workspace,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM workspace_packages WHERE name = $1 AND version = $2 AND workspace = $3",
        )
        .bind(&pkg.name)
        .bind(&pkg.version)
        .bind(&workspace.name)
        .execute(&self.db)
        .await
        .context("failed to remove workspace package from database")?;
        Ok(())
    }

    /// Returns a workspace package matching the name, if any.
    pub async fn get_workspace_package(
        &self,
        name: &str,
        workspace: &Workspace,
    ) -> Result<Option<WorkspacePackageSpec>> {
        let exists =
            sqlx::query_as("SELECT * FROM workspace_packages WHERE name = $1 AND workspace = $2")
                .bind(name)
                .bind(&workspace.name)
                .fetch_optional(&self.db)
                .await
                .context("failed to check for installed workspace package")?;
        Ok(exists)
    }

    /// Adds a registry to the internal state.
    pub async fn add_registry(&self, reg: &Registry) -> Result<()> {
        if !reg.is_initialized() {
            return Err(anyhow!("registry {} is not initialized", &reg.uri));
        }
        if self.registry_exists(&reg.uri.to_string()).await? {
            return Err(anyhow!(
                "registry {} already exists",
                reg.name.as_ref().unwrap()
            ));
        }
        sqlx::query("INSERT INTO registries (name, uri) VALUES ($1, $2)")
            .bind(reg.name.as_ref().unwrap())
            .bind(reg.uri.to_string())
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
        sqlx::query("DELETE FROM registries WHERE uri = $1")
            .bind(uri)
            .execute(&self.db)
            .await
            .context("failed to remove registry from database")?;
        Ok(())
    }

    /// Returns all registries.
    pub async fn registries(&self) -> Result<Vec<Registry>> {
        let registries = sqlx::query_as("SELECT name, uri, last_fetched FROM registries")
            .fetch_all(&self.db)
            .await
            .context("failed to fetch registries from database")?;
        Ok(registries)
    }

    /// Returns true if a registry with this URI exists.
    pub async fn registry_exists(&self, uri: &str) -> Result<bool> {
        let exists = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM registries WHERE uri = $1)")
            .bind(uri)
            .fetch_one(&self.db)
            .await
            .context("failed to check if registry exists in database")?;
        Ok(exists)
    }

    /// Updates the database record of a registry with a new name and last_fetched.
    pub async fn update_registry(&self, reg: &Registry) -> Result<()> {
        if !self.registry_exists(&reg.uri.to_string()).await? {
            return Err(anyhow!("registry {} does not exist", &reg.uri));
        }
        sqlx::query("UPDATE registries SET name = $1, last_fetched = $2 WHERE uri = $3")
            .bind(&reg.name)
            .bind(reg.last_fetched)
            .bind(&reg.uri.to_string())
            .execute(&self.db)
            .await
            .context("failed to update registry last_fetched in database")?;
        Ok(())
    }

    /// Returns all known packages for a registry.
    pub async fn known_packages_for_registry(&self, reg: &Registry) -> Result<Vec<Package>> {
        let pkgs = sqlx::query_as(
            "SELECT * FROM known_packages WHERE registry = $1 ORDER BY name ASC, version DESC",
        )
        .bind(&reg.uri.to_string())
        .fetch_all(&self.db)
        .await
        .context("failed to fetch known packages from database")?;
        Ok(pkgs)
    }

    /// Adds known packages to the database.
    pub async fn add_known_packages(&self, pkgs: &[Package]) -> Result<()> {
        // TODO: We might actually be overwriting another registry's packages. Don't do that.
        if pkgs.iter().any(|p| !p.is_tied_to_registry()) {
            return Err(anyhow!(
                "known packages must be tied to a registry; this is a bug"
            ));
        }
        for pkg in pkgs {
            sqlx::query(
                "INSERT INTO known_packages
                    (name, version, description, homepage, license, registry, source, build)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (name, version)
                    DO UPDATE
                    SET description = $3, homepage = $4, license = $5, registry = $6, source = $7, build = $8
                    WHERE name = $1 AND version = $2",
            )
            .bind(&pkg.name)
            .bind(&pkg.version)
            .bind(&pkg.description)
            .bind(&pkg.homepage)
            .bind(&pkg.license)
            .bind(&pkg.registry)
            .bind(&pkg.source)
            .bind(&pkg.build)
            .execute(&self.db)
            .await
            .context("failed to insert known package into database")?;
        }
        Ok(())
    }

    /// Searches known packages for a query.
    pub async fn search_known_packages(&self, query: &str) -> Result<Vec<Package>> {
        let query = format!("%{}%", query);
        let pkgs = sqlx::query_as(
            r"SELECT *
                FROM known_packages
                WHERE name LIKE $1
                OR description LIKE $1
                OR homepage LIKE $1
                ORDER BY name ASC, version DESC",
        )
        .bind(&query)
        .fetch_all(&self.db)
        .await
        .context("failed to fetch known packages from database")?;
        Ok(pkgs)
    }

    /// Searches know packages for a query, returning only the latest version of each package.
    pub async fn search_known_packages_latest_only(&self, query: &str) -> Result<Vec<Package>> {
        let query = format!("%{}%", query);
        let pkgs = sqlx::query_as(
            r"SELECT *
            FROM (
                SELECT *
                FROM known_packages
                WHERE name LIKE $1
                OR description LIKE $1
                OR homepage LIKE $1
                ORDER BY name ASC, version DESC
            )
            GROUP BY name",
        )
        .bind(&query)
        .fetch_all(&self.db)
        .await
        .context("failed to fetch known packages from database")?;
        Ok(pkgs)
    }

    /// Returns all versions versions of a package, ordered newest to oldest.
    pub async fn known_package_versions(&self, name: &str) -> Result<Vec<String>> {
        let versions = sqlx::query_scalar(
            "SELECT version FROM known_packages WHERE name = $1 ORDER BY version DESC",
        )
        .bind(name)
        .fetch_all(&self.db)
        .await
        .context("failed to fetch known package versions from database")?;
        Ok(versions)
    }

    /// Get the full package from a spec.
    pub async fn get_package(&self, spec: &KnownPackageSpec) -> Result<Package> {
        let pkg = sqlx::query_as("SELECT * FROM known_packages WHERE name = $1 AND version = $2")
            .bind(&spec.name)
            .bind(&spec.version)
            .fetch_one(&self.db)
            .await
            .context("failed to fetch known package from database")?;
        Ok(pkg)
    }

    /// Removes a known package.
    pub async fn remove_known_package(&self, pkg: &KnownPackageSpec) -> Result<()> {
        sqlx::query("DELETE FROM known_packages WHERE name = $1 AND version = $2")
            .bind(&pkg.name)
            .bind(&pkg.version)
            .execute(&self.db)
            .await
            .context("failed to remove known package from database")?;
        Ok(())
    }

    /// Adds a workspace.
    pub async fn add_workspace(&self, workspace: &Workspace) -> Result<()> {
        sqlx::query("INSERT INTO workspaces (name) VALUES ($1)")
            .bind(&workspace.name)
            .execute(&self.db)
            .await
            .context("failed to insert workspace into database")?;
        Ok(())
    }

    /// Removes a workspace.
    pub async fn remove_workspace(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM workspaces WHERE name = $1")
            .bind(name)
            .execute(&self.db)
            .await
            .context("failed to remove workspace from database")?;
        Ok(())
    }

    /// Gets a workspace.
    pub async fn get_workspace(&self, name: &str) -> Result<Option<Workspace>> {
        let workspace = sqlx::query_as("SELECT * FROM workspaces WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.db)
            .await
            .context("failed to fetch workspace from database")?;
        Ok(workspace)
    }

    /// Returns all workspaces.
    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        let workspaces = sqlx::query_as("SELECT * FROM workspaces ORDER BY name ASC")
            .fetch_all(&self.db)
            .await
            .context("failed to fetch workspaces from database")?;
        Ok(workspaces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::OffsetDateTime;

    use crate::{registry::MockFetcher, workspace::test_workspace};

    /// Convenience function to setup the default test state.
    async fn setup_state_with_registry() -> Result<State> {
        let state = State::load(":memory:").await?;
        let mut registry = Registry::new("https://example.invalid/registry");
        registry.initialize(&state, &MockFetcher::default()).await?;
        Ok(state)
    }

    /// Returns a known package spec with the given name and version.
    fn known_package(name: &str, version: &str) -> KnownPackageSpec {
        KnownPackageSpec {
            name: name.to_string(),
            version: version.to_string(),
            requested_version: version.to_string(),
        }
    }

    #[tokio::test]
    async fn test_workspace_package_add_list_remove() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("test-package", "0.1.0");
        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        let packages = state.workspace_packages(&workspace).await?;
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, spec.name);
        assert_eq!(packages[0].version, spec.version);
        state
            .remove_workspace_package(&spec.into(), &workspace)
            .await?;
        let packages = state.workspace_packages(&workspace).await?;
        assert!(packages.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_add_workspace_package_refuses_same_version_twice() -> Result<()> {
        let state = State::load(":memory:").await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("test-package", "0.1.0");
        state.add_installed_package(&spec).await?;
        state.add_workspace_package(&spec, &workspace).await?;
        assert!(state
            .add_workspace_package(&spec, &workspace)
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_is_workspace_package_installed() -> Result<()> {
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("test-package", "0.1.0");
        state.add_installed_package(&spec).await?;
        assert!(state
            .get_workspace_package(&spec.name, &workspace)
            .await?
            .is_none());
        state.add_workspace_package(&spec, &workspace).await?;
        assert!(state
            .get_workspace_package(&spec.name, &workspace)
            .await?
            .is_some());
        Ok(())
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
        state
            .remove_registry("https://example.invalid/registry")
            .await
            .unwrap();
        let registries = state.registries().await.unwrap();
        assert!(registries.is_empty());
    }

    #[tokio::test]
    async fn test_add_registry_refuses_same_name_twice() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_remove_registry_refuses_nonexistent_name() {
        let state = State::load(":memory:").await.unwrap();
        assert!(state.remove_registry("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn test_remove_registry_cascades_to_know_packages() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![
            Package {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "baz".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
        ];
        state.add_known_packages(&pkgs).await.unwrap();
        state
            .remove_registry("https://example.invalid/registry")
            .await
            .unwrap();
        let results = state.search_known_packages("foo").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_registry_exists() {
        let state = setup_state_with_registry().await.unwrap();
        assert!(state
            .registry_exists("https://example.invalid/registry")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_update_registry() {
        let state = State::load(":memory:").await.unwrap();
        let mut registry = Registry::new("https://example.invalid/registry");
        registry
            .initialize(&state, &MockFetcher::default())
            .await
            .unwrap();

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
            Package {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/foo".to_string()),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/bar".to_string()),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "baz".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/baz".to_string()),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
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
        assert_eq!(
            results[0].registry.as_ref().unwrap(),
            "https://example.invalid/registry"
        );
    }

    #[tokio::test]
    async fn test_search_known_packages_latest_only() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![
            Package {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/foo".to_string()),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/bar".to_string()),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "foo".to_string(),
                version: "1.0.1".to_string(),
                description: Some("A test package".to_string()),
                homepage: Some("https://example.invalid/foo".to_string()),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
        ];
        state.add_known_packages(&pkgs).await.unwrap();
        let results = state
            .search_known_packages_latest_only("foo")
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "foo");
        assert_eq!(results[0].version, "1.0.1");
        assert_eq!(results[0].description, Some("A test package".to_string()));
        assert_eq!(
            results[0].homepage,
            Some("https://example.invalid/foo".to_string())
        );
        assert_eq!(
            results[0].registry.as_ref().unwrap(),
            "https://example.invalid/registry"
        );
    }

    #[tokio::test]
    async fn test_add_known_packages_updates_existing() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            description: Some("A test package".to_string()),
            homepage: Some("https://example.invalid/foo".to_string()),
            registry: Some("https://example.invalid/registry".to_string()),
            ..Default::default()
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
        assert_eq!(
            results[0].registry.as_ref().unwrap(),
            "https://example.invalid/registry"
        );
    }

    #[tokio::test]
    async fn test_known_package_versions_is_in_descending_order() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![
            Package {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "foo".to_string(),
                version: "0.1.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "foo".to_string(),
                version: "0.2.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
        ];
        state.add_known_packages(&pkgs).await.unwrap();
        let versions = state.known_package_versions("foo").await.unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0], "1.0.0");
        assert_eq!(versions[1], "0.2.0");
        assert_eq!(versions[2], "0.1.0");
    }

    #[tokio::test]
    async fn test_add_list_remove_workspace() {
        let state = State::load(":memory:").await.unwrap();
        let (_root, workspace) = test_workspace("test").await;
        state.add_workspace(&workspace).await.unwrap();
        let workspaces = state.workspaces().await.unwrap();
        assert_eq!(workspaces.len(), 2);
        assert_eq!(workspaces[1].name, workspace.name);
        state.remove_workspace(&workspace.name).await.unwrap();
        let workspaces = state.workspaces().await.unwrap();
        assert_eq!(workspaces.len(), 1);
    }

    #[tokio::test]
    async fn test_add_workspace_refuses_same_name_twice() {
        let state = State::load(":memory:").await.unwrap();
        let (_root, workspace) = test_workspace("test").await;
        state.add_workspace(&workspace).await.unwrap();
        assert!(state.add_workspace(&workspace).await.is_err());
    }

    #[tokio::test]
    async fn test_get_global_worksace() {
        let state = State::load(":memory:").await.unwrap();
        let workspace = state.get_workspace("global").await.unwrap().unwrap();
        assert_eq!(workspace.name, "global");
    }

    #[tokio::test]
    async fn test_known_packages_for_registry() {
        let state = setup_state_with_registry().await.unwrap();

        let pkgs = vec![
            Package {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
            Package {
                name: "baz".to_string(),
                version: "1.0.0".to_string(),
                registry: Some("https://example.invalid/registry".to_string()),
                ..Default::default()
            },
        ];
        state.add_known_packages(&pkgs).await.unwrap();
        let results = state
            .known_packages_for_registry(&Registry {
                name: None,
                uri: "https://example.invalid/registry".into(),
                last_fetched: None,
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].name, "bar");
        assert_eq!(results[1].name, "baz");
        assert_eq!(results[2].name, "foo");
    }

    #[tokio::test]
    async fn test_remove_known_package() {
        let state = setup_state_with_registry().await.unwrap();

        state
            .remove_known_package(&known_package("test-package", "0.1.0"))
            .await
            .unwrap();
        let results = state
            .known_packages_for_registry(&Registry::default())
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_get_installed_package() -> Result<()> {
        let state = setup_state_with_registry().await?;
        let spec = known_package("test-package", "0.1.0");
        state.add_installed_package(&spec).await?;
        let pkg = state.get_installed_package(&spec).await?.unwrap();
        assert_eq!(pkg.name, spec.name);
        assert_eq!(pkg.version, spec.version);
        Ok(())
    }

    #[tokio::test]
    async fn test_add_known_packages_refuses_untied_packages() -> Result<()> {
        let state = setup_state_with_registry().await?;
        let pkgs = vec![Package {
            name: "test-package".to_string(),
            version: "0.1.0".to_string(),
            ..Default::default()
        }];
        assert!(state.add_known_packages(&pkgs).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_unused_installed_packages() -> Result<()> {
        let state = setup_state_with_registry().await?;
        let (_root, workspace) = test_workspace("global").await;
        let spec = known_package("test-package", "0.1.0");
        state.add_installed_package(&spec).await?;
        assert_eq!(state.unused_installed_packages().await?.len(), 1);
        state.add_workspace_package(&spec, &workspace).await?;
        assert!(state.unused_installed_packages().await?.is_empty());
        state
            .remove_workspace_package(&spec.into(), &workspace)
            .await?;
        assert_eq!(state.unused_installed_packages().await?.len(), 1);
        Ok(())
    }
}

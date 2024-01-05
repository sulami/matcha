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
    pub async fn list_installed_packages(&self) -> Result<Vec<Package>> {
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
    pub async fn resolve_installed_version(&self, pkg: &mut Package) -> Result<()> {
        if !pkg.is_fully_qualified() {
            let installed_versions = self.installed_versions(pkg).await?;
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
        }

        Ok(())
    }

    /// Returns all installed versions of a package, ordered newest to oldest.
    async fn installed_versions(&self, pkg: &Package) -> Result<Vec<String>> {
        let versions = sqlx::query_scalar(
            "SELECT version FROM installed_packages WHERE name = ? ORDER BY version DESC",
        )
        .bind(&pkg.name)
        .fetch_all(&self.db)
        .await
        .context("failed to fetch installed package versions from database")?;
        Ok(versions)
    }
}

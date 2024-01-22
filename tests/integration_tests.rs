use std::{
    path::PathBuf,
    process::{Command as StdCommand, Output, Stdio},
};

use assert_cmd::prelude::*;
use color_eyre::Result;
use tempfile::TempDir;
use tokio::process::Command;

/// Setup required to run a command.
struct TestSetup {
    #[allow(dead_code)]
    config_dir: TempDir,
    state_db: String,
    package_root: TempDir,
    workspace_root: TempDir,
}

impl Default for TestSetup {
    fn default() -> Self {
        let config_dir = TempDir::new().unwrap();
        let state_db = config_dir
            .as_ref()
            .to_owned()
            .join("state.db")
            .to_str()
            .unwrap()
            .to_string();
        Self {
            config_dir,
            state_db,
            package_root: TempDir::new().unwrap(),
            workspace_root: TempDir::new().unwrap(),
        }
    }
}

/// Returns the path to the local test registry.
fn local_test_registry() -> String {
    PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("registry.toml")
        .to_str()
        .unwrap()
        .to_string()
}

/// Runs a command with the provided test setup, returning the result.
async fn run_test_command(setup: &TestSetup, args: &[&str]) -> Result<Output> {
    let mut cmd: Command = StdCommand::cargo_bin("matcha")?.into();
    cmd.args(args)
        .env("MATCHA_STATE_DB", &setup.state_db)
        .env("MATCHA_PACKAGE_ROOT", setup.package_root.path())
        .env("MATCHA_WORKSPACE_ROOT", setup.workspace_root.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = cmd.spawn()?.wait_with_output().await?;
    Ok(output)
}

#[tokio::test]
async fn test_install_a_package() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    Ok(())
}

#[tokio::test]
async fn test_install_two_packages() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(
        &setup,
        &["package", "install", "test-package", "another-package"],
    )
    .await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout.lines().count(), 2);
    assert!(stdout
        .lines()
        .any(|line| line == "test-package@0.1.1 (resolved from *)"));
    assert!(stdout
        .lines()
        .any(|line| line == "another-package@0.2.0 (resolved from *)"));

    Ok(())
}

#[tokio::test]
async fn test_cannot_install_two_different_versions() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@0.1.1"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("conflicting requests for dependency 'test-package'"));

    Ok(())
}

#[tokio::test]
async fn test_install_again_doesnt_upgrade() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "test-package@0.1.0 (resolved from 0.1.0)\n");

    Ok(())
}

#[tokio::test]
async fn test_install_laxer_version() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@~0.1"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    Ok(())
}

#[tokio::test]
async fn test_install_stricter_version() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    Ok(())
}

#[tokio::test]
async fn test_install_package_doesnt_register_if_build_failed() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["package", "install", "failing-build"]).await?;
    assert!(!out.status.success());

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_list_installed_packages() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "test-package@0.1.1 (resolved from *)\n");

    Ok(())
}

#[tokio::test]
async fn test_list_installed_packages_empty() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_show_package() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["registry", "fetch"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "show", "test-package"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(
        stdout,
        format!(
            "test-package@0.1.1\n  Registry: {}\n",
            &local_test_registry()
        )
    );

    Ok(())
}

#[tokio::test]
async fn test_show_unknown_package() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["package", "show", "test-package"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("package test-package is not known"));

    Ok(())
}

#[tokio::test]
async fn test_unninstall_package() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "remove", "test-package"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_cannot_uninstall_unknown_package() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["package", "remove", "test-package"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("package test-package is not installed"));

    Ok(())
}

#[tokio::test]
async fn test_list_registries() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["registry", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, format!("{} (test)\n", &local_test_registry()));

    Ok(())
}

#[tokio::test]
async fn test_cannot_add_duplicate_registry() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains(&format!(
        "registry {} already exists",
        &local_test_registry()
    )));

    Ok(())
}

#[tokio::test]
async fn test_remove_registry() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["registry", "remove", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["registry", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_cannot_remove_unknown_registry() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "remove", "unkonwn"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("registry unkonwn does not exist"));

    Ok(())
}

#[tokio::test]
async fn test_add_workspace() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "global\ntest-workspace\n");

    Ok(())
}

#[tokio::test]
async fn test_cannot_add_duplicate_workspace() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("workspace test-workspace already exists"));

    Ok(())
}

#[tokio::test]
async fn test_cannot_remove_global_workspace() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["workspace", "remove", "global"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("cannot remove global workspace"));

    Ok(())
}

#[tokio::test]
async fn test_cannot_remove_unknown_workspace() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["workspace", "remove", "unknown"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("workspace unknown does not exist"));

    Ok(())
}

#[tokio::test]
async fn test_cannot_use_invalid_workspace_name() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["workspace", "add", "test/workspace"]).await?;
    assert!(!out.status.success());

    let stderr = String::from_utf8(out.stderr)?;
    assert!(stderr.contains("workspace names can contain"));

    Ok(())
}

#[tokio::test]
async fn test_install_different_version_in_workspace() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "test-package@0.1.0"]).await?;
    assert!(out.status.success());

    let out = run_test_command(
        &setup,
        &[
            "package",
            "install",
            "test-package@0.1.1",
            "--workspace",
            "test-workspace",
        ],
    )
    .await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "test-package@0.1.0 (resolved from 0.1.0)\n");

    let out = run_test_command(
        &setup,
        &["package", "list", "--workspace", "test-workspace"],
    )
    .await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "test-package@0.1.1 (resolved from 0.1.1)\n");

    Ok(())
}

#[tokio::test]
async fn test_remove_workspace() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "remove", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "global\n");

    Ok(())
}

#[tokio::test]
async fn test_remove_workspace_with_packages() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "add", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(
        &setup,
        &[
            "package",
            "install",
            "test-package",
            "--workspace",
            "test-workspace",
        ],
    )
    .await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "remove", "test-workspace"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["workspace", "list"]).await?;
    assert!(out.status.success());

    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, "global\n");

    Ok(())
}

#[tokio::test]
async fn test_garbage_collect_installed_packages() -> Result<()> {
    let setup = TestSetup::default();

    let out = run_test_command(&setup, &["registry", "add", &local_test_registry()]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "install", "package-with-artifact"]).await?;
    assert!(out.status.success());

    let out = run_test_command(&setup, &["package", "remove", "package-with-artifact"]).await?;
    assert!(out.status.success());

    assert!(setup
        .package_root
        .path()
        .join("package-with-artifact")
        .join("0.1.0")
        .try_exists()?);

    let out = run_test_command(&setup, &["package", "garbage-collect"]).await?;
    assert!(out.status.success());

    assert!(!setup
        .package_root
        .path()
        .join("package-with-artifact")
        .join("0.1.0")
        .try_exists()?);

    Ok(())
}

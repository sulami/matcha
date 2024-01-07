use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use tempfile::TempDir;
use tokio::{fs::File, io::AsyncWriteExt, process::Command};
use url::Url;

use crate::{download::download_stream, manifest::Package};

/// Downloads and builds a package.
pub async fn download_build_package(package: &Package) -> Result<()> {
    let Some(source) = &package.source else {
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
    if let Some(build) = &package.build {
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

    Ok(())
}

use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Client;
use tokio::sync::watch::Sender;

/// Downloads a file from a URL, and returns the bytes.
///
/// Also accepts a progress channel, which will be sent the ratio of bytes
/// downloaded to total bytes.
pub async fn download_file(url: &str, progress: Sender<usize>) -> Result<Vec<u8>> {
    let client = Client::new();
    let resp = client.get(url).send().await?;

    let content_length = resp.content_length().unwrap_or(0) as usize;
    let mut bytes = vec![];
    let mut downloaded = 0;
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        downloaded += chunk.len();
        progress.send(downloaded / content_length)?;
    }

    Ok(bytes)
}

use anyhow::Result;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::Client;
use tokio::sync::watch::Sender;

/// Downloads a file from a URL, and returns the bytes.
///
/// Also accepts a progress channel, which will be sent the ratio of bytes
/// downloaded to total bytes.
pub async fn download_file(url: &str, progress: Option<Sender<usize>>) -> Result<Vec<u8>> {
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
        if let Some(tx) = &progress {
            tx.send(downloaded / content_length)?;
        }
    }

    Ok(bytes)
}

/// Downloads a file from a URL, and returns the content length and a stream of bytes.
pub async fn download_stream(
    url: &str,
) -> Result<(usize, impl Stream<Item = reqwest::Result<Bytes>>)> {
    let client = Client::new();
    let resp = client.get(url).send().await?;

    let content_length = resp.content_length().unwrap_or(0) as usize;
    let stream = resp.bytes_stream();

    Ok((content_length, stream))
}

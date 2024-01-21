use bytes::Bytes;
use color_eyre::Result;
use futures_util::{Stream, StreamExt};
use reqwest::Client;
use tracing::instrument;

/// A trait for downloading files.
pub trait Downloader {
    /// Downloads a file from a URL, and returns the bytes.
    async fn download_file(&self, url: &str) -> Result<Vec<u8>>;
    /// Downloads a file from a URL, and returns the content length and a stream of bytes.
    async fn download_stream(
        &self,
        url: &str,
    ) -> Result<(usize, impl Stream<Item = reqwest::Result<Bytes>>)>;
}

/// The default downloader, which uses reqwest.
pub struct DefaultDownloader;

impl Downloader for DefaultDownloader {
    async fn download_file(&self, url: &str) -> Result<Vec<u8>> {
        download_file(url).await
    }

    async fn download_stream(
        &self,
        url: &str,
    ) -> Result<(usize, impl Stream<Item = reqwest::Result<Bytes>>)> {
        download_stream(url).await
    }
}

/// Downloads a file from a URL, and returns the bytes.
#[instrument]
pub async fn download_file(url: &str) -> Result<Vec<u8>> {
    let (_, mut stream) = download_stream(url).await?;
    let mut bytes = vec![];

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

/// Downloads a file from a URL, and returns the content length and a stream of bytes.
#[instrument]
pub async fn download_stream(
    url: &str,
) -> Result<(usize, impl Stream<Item = reqwest::Result<Bytes>>)> {
    let client = Client::new();
    let resp = client
        .get(url)
        .header("User-Agent", "matcha")
        .send()
        .await?;

    let content_length = resp.content_length().unwrap_or(0) as usize;
    let stream = resp.bytes_stream();

    Ok((content_length, stream))
}

#[cfg(test)]
pub struct MockDownloader {
    pub file: Vec<u8>,
}

#[cfg(test)]
impl MockDownloader {
    pub fn new(file: Vec<u8>) -> Self {
        Self { file }
    }
}

#[cfg(test)]
impl Downloader for MockDownloader {
    async fn download_file(&self, _: &str) -> Result<Vec<u8>> {
        Ok(self.file.clone())
    }

    async fn download_stream(
        &self,
        _: &str,
    ) -> Result<(usize, impl Stream<Item = reqwest::Result<Bytes>>)> {
        Ok((
            self.file.len(),
            futures_util::stream::once(async move { Ok(Bytes::from(self.file.clone())) }),
        ))
    }
}

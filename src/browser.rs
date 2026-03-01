use anyhow::{Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;
use tracing::{debug, info};

/// Connect to an already-running Chrome instance via CDP.
///
/// Chrome must be started with `--remote-debugging-port=<port>`, e.g.:
///   google-chrome --remote-debugging-port=9222
pub async fn connect(port: u16) -> Result<Browser> {
    let url = format!("http://127.0.0.1:{port}");
    info!(port = port, "connecting to Chrome via CDP");
    let (browser, mut handler) = Browser::connect(&url)
        .await
        .with_context(|| format!("Failed to connect to Chrome on port {port}. Is Chrome running with --remote-debugging-port={port}?"))?;

    tokio::spawn(async move { while handler.next().await.is_some() {} });

    info!(port = port, "connected to Chrome");
    Ok(browser)
}

/// Launch a new headless Chrome instance.
pub async fn launch() -> Result<Browser> {
    info!("launching headless Chrome");
    let config = BrowserConfig::builder()
        .no_sandbox()
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build browser config: {e}"))?;

    let (browser, mut handler) = Browser::launch(config)
        .await
        .context("Failed to launch Chrome. Is Chrome/Chromium installed?")?;

    tokio::spawn(async move { while handler.next().await.is_some() {} });

    info!("headless Chrome launched");
    Ok(browser)
}

/// Navigate to a URL and return the page's rendered HTML content and final URL.
pub async fn fetch_page(browser: &Browser, url: &str) -> Result<(String, String)> {
    debug!(url = %url, "fetching page");
    let page = browser
        .new_page(url)
        .await
        .with_context(|| format!("Failed to navigate to {url}"))?;

    page.wait_for_navigation().await.ok();

    let html = page.content().await.context("Failed to get page content")?;

    let final_url = page
        .url()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| url.to_string());

    debug!(final_url = %final_url, html_len = html.len(), "page fetched");
    Ok((html, final_url))
}

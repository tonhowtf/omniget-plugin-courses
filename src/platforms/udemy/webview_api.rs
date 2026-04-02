use std::sync::Arc;
use anyhow::anyhow;

pub async fn create_api_webview(
    _portal_name: &str,
) -> anyhow::Result<()> {
    Err(anyhow!("Webview API not available in plugin mode"))
}

pub async fn fetch_curriculum_via_webview(
    _course_id: u64,
    _portal_name: &str,
) -> anyhow::Result<String> {
    Err(anyhow!("Webview API not available in plugin mode"))
}

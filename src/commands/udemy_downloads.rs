use std::sync::Arc;

use serde::Serialize;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::platforms::udemy::api::{self, UdemyCourse, UdemyCurriculum};
use crate::platforms::udemy::downloader::UdemyDownloader;



#[derive(Clone, Serialize)]
struct UdemyDownloadCompleteEvent {
    course_name: String,
    success: bool,
    error: Option<String>,
    drm_skipped: u32,
}

async fn fetch_curriculum_via_webview(
    _host: &std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    _course_id: u64,
    _portal: &str,
) -> Result<crate::platforms::udemy::api::UdemyCurriculum, String> {
    Err("Webview curriculum fetch not available in plugin mode".into())
}

async fn fetch_curriculum_via_api(
    plugin: &crate::CoursesPlugin,
    course_id: u64,
    portal_name: &str,
) -> Result<UdemyCurriculum, String> {
    let client = {
        let guard = plugin.udemy_session.lock().await;
        let session = guard.as_ref().ok_or("not_authenticated")?;
        session.client.clone()
    };

    let url = format!(
        "https://{}.udemy.com/api-2.0/courses/{}/subscriber-curriculum-items/?fields[lecture]=title,object_index,asset,supplementary_assets&fields[quiz]=title,object_index,type&fields[practice]=title,object_index&fields[chapter]=title,object_index&fields[asset]=title,filename,asset_type,status,is_external,media_license_token,course_is_drmed,media_sources,captions,stream_urls,download_urls,external_url,body&page_size=200",
        portal_name, course_id
    );

    tracing::info!("[udemy-api] fetching curriculum via direct API for course {}", course_id);

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("API returned status {}", resp.status()));
    }

    let body = resp.text().await.map_err(|e| format!("Read body failed: {}", e))?;

    let mut data: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("JSON parse error: {}", e))?;

    let mut all_results = data.get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    loop {
        let next_url = data.get("next").and_then(|n| n.as_str()).map(|s| s.to_string());
        match next_url {
            Some(next) if !next.is_empty() => {
                tracing::info!("[udemy-api] fetching next curriculum page via direct API");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;

                let page_resp = client
                    .get(&next)
                    .send()
                    .await
                    .map_err(|e| format!("API page request failed: {}", e))?;

                if !page_resp.status().is_success() {
                    break;
                }

                let page_body = page_resp.text().await
                    .map_err(|e| format!("Read page body failed: {}", e))?;

                let page_data: serde_json::Value = serde_json::from_str(&page_body)
                    .map_err(|e| format!("JSON parse error on page: {}", e))?;

                if let Some(new_results) = page_data.get("results").and_then(|r| r.as_array()) {
                    all_results.extend(new_results.iter().cloned());
                }

                data = page_data;
            }
            _ => break,
        }
    }

    tracing::info!("[udemy-api] curriculum fetched via direct API: {} items total", all_results.len());

    api::parse_curriculum(course_id, &all_results).map_err(|e| e.to_string())
}


pub async fn start_udemy_course_download(
    host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    plugin: &crate::CoursesPlugin,
    course_json: String,
    output_dir: String,
) -> Result<String, String> {
    let course: UdemyCourse =
        serde_json::from_str(&course_json).map_err(|e| format!("Invalid JSON: {}", e))?;

    let course_name = course.title.clone();
    let course_id = course.id;
    let session = plugin.udemy_session.clone();
    let active = plugin.active_downloads.clone();

    let cancel_token = CancellationToken::new();

    {
        let mut map = active.lock().await;
        if map.contains_key(&course_id) {
            return Err("Download already in progress for this course".to_string());
        }
        map.insert(course_id, cancel_token.clone());
    }

    let portal = {
        let guard = plugin.udemy_session.lock().await;
        guard
            .as_ref()
            .map(|s| s.portal_name.clone())
            .unwrap_or_else(|| "www".into())
    };

    let curriculum = if portal != "www" {
        match fetch_curriculum_via_api(&plugin, course_id, &portal).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[udemy-api] direct API curriculum failed for portal={}, falling back to webview: {}", portal, e);
                                                match fetch_curriculum_via_webview(&host, course_id, &portal).await {
                    Ok(c) => c,
                    Err(e2) => {
                        active.lock().await.remove(&course_id);
                        return Err(format!("Failed to fetch curriculum: {}", e2));
                    }
                }
            }
        }
    } else {
                        match fetch_curriculum_via_webview(&host, course_id, &portal).await {
            Ok(c) => c,
            Err(e) => {
                active.lock().await.remove(&course_id);
                return Err(format!("Failed to fetch curriculum: {}", e));
            }
        }
    };

    if curriculum.drm_video_lectures > 0
        && curriculum.drm_video_lectures == curriculum.total_video_lectures
        && curriculum.total_video_lectures > 0
    {
        active.lock().await.remove(&course_id);
        return Err("drm_protected".to_string());
    }

    let settings = crate::settings_reader::load_app_settings();

    tokio::spawn(async move {
        let downloader = UdemyDownloader::new(
            session,
            settings.advanced.max_concurrent_segments,
            settings.advanced.max_retries,
        );
        let (tx, mut rx) = mpsc::channel(32);

        let host_clone = host.clone();
        let progress_forwarder = tokio::spawn(async move {
            while let Some(progress) = rx.recv().await {
                let _ = host_clone.emit_event("udemy-download-progress", serde_json::to_value(&progress).unwrap_or_default());
            }
        });

        let result = downloader
            .download_full_course(&course, &output_dir, curriculum, tx, cancel_token)
            .await;

        let _ = progress_forwarder.await;

        {
            let mut map = active.lock().await;
            map.remove(&course_id);
        }

        match result {
            Ok(drm_skipped) => {
                let _ = host.emit_event(
                    "udemy-download-complete", serde_json::to_value(&UdemyDownloadCompleteEvent {
                        course_name: course.title,
                        success: true,
                        error: None,
                        drm_skipped,
                    },).unwrap_or_default());
            }
            Err(e) => {
                tracing::error!("[udemy] download error for '{}': {}", course.title, e);
                let _ = host.emit_event(
                    "udemy-download-complete", serde_json::to_value(&UdemyDownloadCompleteEvent {
                        course_name: course.title,
                        success: false,
                        error: Some(e.to_string()),
                        drm_skipped: 0,
                    },).unwrap_or_default());
            }
        }
    });

    Ok(format!("Download started: {}", course_name))
}


pub async fn cancel_udemy_course_download(
    plugin: &crate::CoursesPlugin,
    course_id: u64,
) -> Result<(), String> {
    let map = plugin.active_downloads.lock().await;
    if let Some(token) = map.get(&course_id) {
        token.cancel();
        Ok(())
    } else {
        Err("No active download for this course".to_string())
    }
}

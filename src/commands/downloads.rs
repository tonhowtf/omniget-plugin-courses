use serde::Serialize;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::platforms::hotmart::api::Course;
use crate::platforms::hotmart::downloader::HotmartDownloader;



#[derive(Clone, Serialize)]
struct DownloadCompleteEvent {
    course_name: String,
    success: bool,
    error: Option<String>,
}


pub async fn start_course_download(
    host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    plugin: &crate::CoursesPlugin,
    course_json: String,
    output_dir: String,
) -> Result<String, String> {
    let course: Course =
        serde_json::from_str(&course_json).map_err(|e| format!("Invalid JSON: {}", e))?;

    let course_name = course.name.clone();
    let course_id = course.id;
    let session = plugin.hotmart_session.clone();
    let active = plugin.active_downloads.clone();

    let cancel_token = CancellationToken::new();

    {
        let mut map = active.lock().await;
        if map.contains_key(&course_id) {
            return Err("Download already in progress for this course".to_string());
        }
        map.insert(course_id, cancel_token.clone());
    }

    let settings = crate::settings_reader::load_app_settings();

    tokio::spawn(async move {
        let downloader = HotmartDownloader::new(
            session,
            settings.download,
            settings.advanced.max_concurrent_segments,
            settings.advanced.max_retries,
            settings.advanced.concurrent_fragments,
        );
        let (tx, mut rx) = mpsc::channel(32);

        let host_clone = host.clone();
        let progress_forwarder = tokio::spawn(async move {
            while let Some(progress) = rx.recv().await {
                let _ = host_clone.emit_event("download-progress", serde_json::to_value(&progress).unwrap_or_default());
            }
        });

        let result = downloader
            .download_full_course(&course, &output_dir, tx, cancel_token)
            .await;

        let _ = progress_forwarder.await;

        {
            let mut map = active.lock().await;
            map.remove(&course_id);
        }

        match result {
            Ok(()) => {
                let _ = host.emit_event(
                    "download-complete", serde_json::to_value(&DownloadCompleteEvent {
                        course_name: course.name,
                        success: true,
                        error: None,
                    },).unwrap_or_default());
            }
            Err(e) => {
                tracing::error!("Download error for '{}': {}", course.name, e);
                let _ = host.emit_event(
                    "download-complete", serde_json::to_value(&DownloadCompleteEvent {
                        course_name: course.name,
                        success: false,
                        error: Some(e.to_string()),
                    },).unwrap_or_default());
            }
        }
    });

    Ok(format!("Download started: {}", course_name))
}


pub async fn cancel_course_download(
    plugin: &crate::CoursesPlugin,
    course_id: u64,
) -> Result<String, String> {
    let mut map = plugin.active_downloads.lock().await;
    match map.remove(&course_id) {
        Some(token) => {
            token.cancel();
            Ok("Download cancelled".to_string())
        }
        None => Err("No active download for this course".to_string()),
    }
}


pub async fn get_active_downloads(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<u64>, String> {
    let map = plugin.active_downloads.lock().await;
    Ok(map.keys().copied().collect())
}

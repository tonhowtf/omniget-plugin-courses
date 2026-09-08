
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
    videos_downloaded: u32,
    videos_already_present: u32,
    no_media: u32,
    failed: u32,
    lectures_processed: u32,
    skipped: UdemySkippedLectures,
}

#[derive(Clone, Serialize, Default)]
struct UdemySkippedLectures {
    drm: Vec<String>,
    no_media: Vec<String>,
    failed: Vec<String>,
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
    let session = {
        let guard = plugin.udemy_session.lock().await;
        guard.clone().ok_or("not_authenticated")?
    };

    api::get_course_curriculum(&session, portal_name, course_id)
        .await
        .map_err(|e| e.to_string())
}

pub async fn udemy_get_curriculum(
    plugin: &crate::CoursesPlugin,
    course_id: u64,
) -> Result<api::UdemyCurriculumSummary, String> {
    let portal = {
        let guard = plugin.udemy_session.lock().await;
        guard
            .as_ref()
            .map(|s| s.portal_name.clone())
            .unwrap_or_else(|| "www".into())
    };
    let curriculum = fetch_curriculum_via_api(plugin, course_id, &portal).await?;
    Ok(api::summarize_curriculum(&curriculum))
}


pub async fn start_udemy_course_download(
    host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    plugin: &crate::CoursesPlugin,
    course_json: String,
    output_dir: String,
    chapter_filter_raw: Option<String>,
    section_ids: Option<Vec<u64>>,
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

    let curriculum = match fetch_curriculum_via_api(&plugin, course_id, &portal).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[udemy] direct API curriculum failed for portal={}, falling back to webview: {}", portal, e);
            match fetch_curriculum_via_webview(&host, course_id, &portal).await {
                Ok(c) => c,
                Err(e2) => {
                    active.lock().await.remove(&course_id);
                    return Err(format!("Failed to fetch curriculum: API={}, Webview={}", e, e2));
                }
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
    let target_quality = crate::platforms::udemy::downloader::parse_quality_pref(
        &settings.download.video_quality,
    );
    let continuous_lecture_numbers = settings.download.continuous_lecture_numbers;
    let chapter_filter = chapter_filter_raw
        .as_deref()
        .map(crate::platforms::udemy::api::parse_chapter_filter)
        .unwrap_or_default();
    let section_ids: std::collections::HashSet<u64> = section_ids
        .unwrap_or_default()
        .into_iter()
        .collect();
    if !section_ids.is_empty() {
        let known: Vec<u64> = curriculum.chapters.iter().map(|c| c.id).collect();
        let unknown: Vec<u64> = section_ids.iter().copied().filter(|id| !known.contains(id)).collect();
        if !unknown.is_empty() {
            active.lock().await.remove(&course_id);
            return Err(format!("unknown section ids for this course: {:?}", unknown));
        }
        tracing::info!("[udemy] downloading {} of {} sections", section_ids.len(), known.len());
    }

    let download_captions = settings.download.download_subtitles;
    let caption_locale = settings.download.caption_locale.clone();

    let course_locale = if download_captions {
        match course.locale.clone().filter(|l| !l.trim().is_empty()) {
            Some(l) => Some(l),
            None => {
                let session_snapshot = { plugin.udemy_session.lock().await.clone() };
                match session_snapshot {
                    Some(s) => match api::get_course_locale(&s, &portal, course_id).await {
                        Ok(l) => l,
                        Err(e) => {
                            tracing::warn!("[udemy] failed to fetch course locale for {}: {}", course_id, e);
                            None
                        }
                    },
                    None => None,
                }
            }
        }
    } else {
        None
    };

    if download_captions {
        tracing::info!(
            "[udemy] caption selection inputs: requested='{}', course_locale='{}'",
            caption_locale,
            course_locale.as_deref().unwrap_or("unknown")
        );
    }

    tokio::spawn(async move {
        let downloader = UdemyDownloader::new(
            session,
            settings.advanced.max_concurrent_segments,
            settings.advanced.max_retries,
            settings.download.keep_vtt,
            target_quality,
            continuous_lecture_numbers,
            chapter_filter,
            section_ids,
            download_captions,
            caption_locale,
            course_locale,
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
            Ok(summary) => {
                if summary.drm_skipped > 0 {
                    let _ = host.emit_event("udemy-download-progress", serde_json::json!({
                        "courseId": course_id,
                        "type": "drm_warning",
                        "drm_skipped": summary.drm_skipped,
                        "message": format!("{} lectures have DRM protection and were skipped", summary.drm_skipped)
                    }));
                }
                if summary.no_media + summary.failed > 0 {
                    let _ = host.emit_event("udemy-download-progress", serde_json::json!({
                        "courseId": course_id,
                        "type": "skipped_warning",
                        "no_media": summary.no_media,
                        "failed": summary.failed,
                        "titles": summary.no_media_titles.iter().chain(summary.failed_titles.iter()).cloned().collect::<Vec<_>>(),
                        "message": format!(
                            "{} lectures had no downloadable media and {} failed",
                            summary.no_media, summary.failed
                        )
                    }));
                }
                let _ = host.emit_event(
                    "udemy-download-complete", serde_json::to_value(&UdemyDownloadCompleteEvent {
                        course_name: course.title,
                        success: true,
                        error: None,
                        drm_skipped: summary.drm_skipped,
                        videos_downloaded: summary.videos_downloaded,
                        videos_already_present: summary.videos_already_present,
                        no_media: summary.no_media,
                        failed: summary.failed,
                        lectures_processed: summary.lectures_processed,
                        skipped: UdemySkippedLectures {
                            drm: summary.drm_skipped_titles,
                            no_media: summary.no_media_titles,
                            failed: summary.failed_titles,
                        },
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
                        videos_downloaded: 0,
                        videos_already_present: 0,
                        no_media: 0,
                        failed: 0,
                        lectures_processed: 0,
                        skipped: UdemySkippedLectures::default(),
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

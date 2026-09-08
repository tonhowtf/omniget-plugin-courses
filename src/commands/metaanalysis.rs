use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use serde::Serialize;

use tokio_util::sync::CancellationToken;

use crate::platforms::metaanalysis::api::{self, MetaCourse};
use crate::platforms::metaanalysis::downloader;

const SESSION_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const COURSES_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Serialize)]
struct MetaDownloadCompleteEvent {
    course_name: String,
    success: bool,
    error: Option<String>,
}

pub async fn metaanalysis_login(
    plugin: &crate::CoursesPlugin,
    email: String,
    password: String,
) -> Result<String, String> {
    let _ = api::delete_saved_session().await;
    plugin.metaanalysis_session.lock().await.take();
    *plugin.metaanalysis_session_validated_at.lock().await = None;
    *plugin.metaanalysis_courses_cache.lock().await = None;

    match api::authenticate(&email, &password).await {
        Ok(session) => {
            let response_email = session.email.clone();
            let _ = api::save_session(&session).await;
            *plugin.metaanalysis_session.lock().await = Some(session);
            *plugin.metaanalysis_session_validated_at.lock().await = Some(Instant::now());
            Ok(response_email)
        }
        Err(e) => {
            tracing::error!("[metaanalysis] login failed: {}", e);
            Err(format!("Login failed: {}", e))
        }
    }
}

pub async fn metaanalysis_check_session(
    plugin: &crate::CoursesPlugin,
) -> Result<String, String> {
    let has_memory_session = plugin.metaanalysis_session.lock().await.is_some();

    if !has_memory_session {
        match api::load_session().await {
            Ok(Some(session)) => {
                *plugin.metaanalysis_session.lock().await = Some(session);
            }
            _ => return Err("not_authenticated".to_string()),
        }
    }

    let session = {
        let guard = plugin.metaanalysis_session.lock().await;
        guard
            .as_ref()
            .ok_or_else(|| "not_authenticated".to_string())?
            .clone()
    };

    {
        let validated_at = plugin.metaanalysis_session_validated_at.lock().await;
        if let Some(at) = *validated_at {
            if at.elapsed() < SESSION_COOLDOWN {
                return Ok(session.email.clone());
            }
        }
    }

    match api::validate(&session).await {
        Ok(email) => {
            // Persist any refreshed token so the next launch stays logged in.
            let _ = api::save_session(&session).await;
            *plugin.metaanalysis_session_validated_at.lock().await = Some(Instant::now());
            Ok(email)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("401") || msg.contains("403") {
                plugin.metaanalysis_session.lock().await.take();
                *plugin.metaanalysis_session_validated_at.lock().await = None;
                *plugin.metaanalysis_courses_cache.lock().await = None;
                let _ = api::delete_saved_session().await;
                Err("session_expired".to_string())
            } else {
                Err(format!("session_check_failed: {}", msg))
            }
        }
    }
}

pub async fn metaanalysis_logout(plugin: &crate::CoursesPlugin) -> Result<(), String> {
    let _ = api::delete_saved_session().await;
    plugin.metaanalysis_session.lock().await.take();
    *plugin.metaanalysis_session_validated_at.lock().await = None;
    *plugin.metaanalysis_courses_cache.lock().await = None;
    Ok(())
}

async fn fetch_courses(plugin: &crate::CoursesPlugin) -> Result<Vec<MetaCourse>, String> {
    let session = {
        let guard = plugin.metaanalysis_session.lock().await;
        guard
            .as_ref()
            .ok_or_else(|| "Not authenticated. Please log in first.".to_string())?
            .clone()
    };

    let courses = tokio::time::timeout(Duration::from_secs(60), api::list_courses(&session))
        .await
        .map_err(|_| "Timeout loading courses (60s)".to_string())?
        .map_err(|e| e.to_string())?;

    *plugin.metaanalysis_courses_cache.lock().await = Some(crate::state::MetaCoursesCache {
        courses: courses.clone(),
        fetched_at: Instant::now(),
    });

    Ok(courses)
}

pub async fn metaanalysis_list_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<MetaCourse>, String> {
    {
        let cache = plugin.metaanalysis_courses_cache.lock().await;
        if let Some(ref cached) = *cache {
            if cached.fetched_at.elapsed() < COURSES_CACHE_TTL {
                return Ok(cached.courses.clone());
            }
        }
    }
    fetch_courses(plugin).await
}

pub async fn metaanalysis_refresh_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<MetaCourse>, String> {
    *plugin.metaanalysis_courses_cache.lock().await = None;
    fetch_courses(plugin).await
}

pub async fn start_metaanalysis_course_download(
    host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    plugin: &crate::CoursesPlugin,
    course_json: String,
    output_dir: String,
) -> Result<String, String> {
    let course: MetaCourse =
        serde_json::from_str(&course_json).map_err(|e| format!("Invalid JSON: {}", e))?;

    let course_name = course.name.clone();
    let course_key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        course.id.hash(&mut hasher);
        hasher.finish()
    };
    let active = plugin.active_downloads.clone();
    let cancel_token = CancellationToken::new();

    {
        let mut map = active.lock().await;
        if map.contains_key(&course_key) {
            return Err("Download already in progress for this course".to_string());
        }
        map.insert(course_key, cancel_token.clone());
    }

    let session = {
        let guard = plugin.metaanalysis_session.lock().await;
        guard
            .as_ref()
            .ok_or_else(|| "Not authenticated. Please log in first.".to_string())?
            .clone()
    };

    tokio::spawn(async move {
        let result =
            downloader::download_full_course(&host, &session, &course, &output_dir, cancel_token)
                .await;

        {
            let mut map = active.lock().await;
            map.remove(&course_key);
        }

        let event = match result {
            Ok(()) => MetaDownloadCompleteEvent {
                course_name: course.name.clone(),
                success: true,
                error: None,
            },
            Err(e) => {
                tracing::error!("[metaanalysis] download error for '{}': {}", course.name, e);
                MetaDownloadCompleteEvent {
                    course_name: course.name.clone(),
                    success: false,
                    error: Some(e.to_string()),
                }
            }
        };
        let _ = host.emit_event(
            "download-complete",
            serde_json::to_value(&event).unwrap_or_default(),
        );
    });

    Ok(format!("Download started: {}", course_name))
}

pub async fn cancel_metaanalysis_course_download(
    plugin: &crate::CoursesPlugin,
    course_id: &str,
) -> Result<String, String> {
    let key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        course_id.hash(&mut hasher);
        hasher.finish()
    };
    let mut map = plugin.active_downloads.lock().await;
    match map.remove(&key) {
        Some(token) => {
            token.cancel();
            Ok("Download cancelled".to_string())
        }
        None => Err("No active download for this course".to_string()),
    }
}

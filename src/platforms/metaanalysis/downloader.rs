use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::anyhow;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use omniget_core::core::filename;

use super::api::{self, MetaCourse, MetaLesson, MetaSession};

/// Progress payload — matches the shape the app's download listener expects
/// (the same one Hotmart/Kiwify emit), so course downloads surface in the UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetaCourseDownloadProgress {
    pub course_id: String,
    pub course_name: String,
    pub percent: f64,
    pub current_module: String,
    #[serde(rename = "current_page")]
    pub current_lesson: String,
    pub downloaded_bytes: u64,
    #[serde(rename = "total_pages")]
    pub total_lessons: u32,
    #[serde(rename = "completed_pages")]
    pub completed_lessons: u32,
    pub total_modules: u32,
    pub current_module_index: u32,
}

struct DownloadCtx {
    course_id: String,
    course_name: String,
    total_lessons: usize,
    total_modules: usize,
    total_bytes: Arc<AtomicU64>,
    completed: Arc<AtomicUsize>,
}

impl DownloadCtx {
    fn emit(
        &self,
        host: &Arc<dyn omniget_plugin_sdk::PluginHost>,
        module_name: &str,
        lesson_name: &str,
        module_index: usize,
    ) {
        let done = self.completed.load(Ordering::Relaxed);
        let percent = if self.total_lessons == 0 {
            100.0
        } else {
            done as f64 / self.total_lessons as f64 * 100.0
        };
        let _ = host.emit_event(
            "download-progress",
            serde_json::to_value(MetaCourseDownloadProgress {
                course_id: self.course_id.clone(),
                course_name: self.course_name.clone(),
                percent,
                current_module: module_name.to_string(),
                current_lesson: lesson_name.to_string(),
                downloaded_bytes: self.total_bytes.load(Ordering::Relaxed),
                total_lessons: self.total_lessons as u32,
                completed_lessons: done as u32,
                total_modules: self.total_modules as u32,
                current_module_index: module_index as u32,
            })
            .unwrap_or_default(),
        );
    }
}

pub async fn download_full_course(
    host: &Arc<dyn omniget_plugin_sdk::PluginHost>,
    session: &MetaSession,
    course: &MetaCourse,
    output_dir: &str,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    let modules = api::get_course_content(session, &course.id).await?;
    if modules.is_empty() {
        return Err(anyhow!(
            "'{}' has no modules available for download",
            course.name
        ));
    }

    let course_dir = format!(
        "{}/{}",
        output_dir,
        filename::sanitize_path_component(&course.name)
    );
    std::fs::create_dir_all(&course_dir)?;

    if omniget_core::core::course_utils::is_course_complete(&course_dir) {
        return Ok(());
    }

    let total_lessons: usize = modules
        .iter()
        .flat_map(|m| m.submodules.iter())
        .map(|s| s.lessons.len())
        .sum();

    let ctx = DownloadCtx {
        course_id: course.id.clone(),
        course_name: course.name.clone(),
        total_lessons,
        total_modules: modules.len(),
        total_bytes: Arc::new(AtomicU64::new(0)),
        completed: Arc::new(AtomicUsize::new(0)),
    };

    // Public client for Bunny CDN and attachment downloads.
    let public = session.public_client()?;

    ctx.emit(host, "Starting...", "", 0);

    for (mi, module) in modules.iter().enumerate() {
        if cancel_token.is_cancelled() {
            return Err(anyhow!("Download cancelled by user"));
        }

        let mod_name = filename::sanitize_path_component(&module.title);
        let mod_dir = format!("{}/{}. {}", course_dir, mi + 1, mod_name);
        std::fs::create_dir_all(&mod_dir)?;

        for (si, submodule) in module.submodules.iter().enumerate() {
            if cancel_token.is_cancelled() {
                return Err(anyhow!("Download cancelled by user"));
            }

            // Submodules with a real title get their own folder; when a module
            // has a single unnamed submodule the lessons stay directly under it.
            let sub_dir = if submodule.title.trim().is_empty() {
                mod_dir.clone()
            } else {
                let sub_name = filename::sanitize_path_component(&submodule.title);
                format!("{}/{}. {}", mod_dir, si + 1, sub_name)
            };
            std::fs::create_dir_all(&sub_dir)?;

            for (li, lesson) in submodule.lessons.iter().enumerate() {
                if cancel_token.is_cancelled() {
                    return Err(anyhow!("Download cancelled by user"));
                }

                download_lesson(
                    host,
                    &public,
                    &ctx,
                    &module.title,
                    mi + 1,
                    &sub_dir,
                    li + 1,
                    lesson,
                    &cancel_token,
                )
                .await;

                ctx.completed.fetch_add(1, Ordering::Relaxed);
                ctx.emit(host, &module.title, &lesson.title, mi + 1);
            }
        }
    }

    if cancel_token.is_cancelled() {
        return Err(anyhow!("Download cancelled by user"));
    }

    omniget_core::core::course_utils::mark_course_complete(&course_dir)
        .await
        .ok();

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_lesson(
    _host: &Arc<dyn omniget_plugin_sdk::PluginHost>,
    public: &reqwest::Client,
    ctx: &DownloadCtx,
    _module_title: &str,
    _module_index: usize,
    parent_dir: &str,
    lesson_index: usize,
    lesson: &MetaLesson,
    cancel_token: &CancellationToken,
) {
    let lesson_name = filename::sanitize_path_component(&lesson.title);
    let lesson_dir = format!("{}/{}. {}", parent_dir, lesson_index, lesson_name);
    if let Err(e) = std::fs::create_dir_all(&lesson_dir) {
        tracing::error!("[metaanalysis] cannot create '{}': {}", lesson_dir, e);
        return;
    }

    // Lesson body (HTML).
    if let Some(ref content) = lesson.content {
        omniget_core::core::course_utils::save_description(&lesson_dir, content, "html")
            .await
            .ok();
    }

    // Video (Bunny Stream).
    if let Some(ref video_url) = lesson.video_url {
        let video_path = format!("{}/{}. {}.mp4", lesson_dir, lesson_index, lesson_name);
        if !file_exists_nonempty(&video_path) {
            match download_bunny_video(public, video_url, &video_path, cancel_token).await {
                Ok(size) => {
                    ctx.total_bytes.fetch_add(size, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::error!(
                        "[metaanalysis] video download failed for '{}': {}",
                        lesson.title,
                        e
                    );
                    let _ = std::fs::remove_file(&video_path);
                }
            }
        }
    }

    // Attachments.
    for file in &lesson.files {
        if cancel_token.is_cancelled() {
            return;
        }
        let file_name = filename::sanitize_path_component(&file.name);
        let file_path = format!("{}/{}", lesson_dir, file_name);
        if file_exists_nonempty(&file_path) {
            continue;
        }
        match download_file_direct(public, &file.url, &file_path, cancel_token).await {
            Ok(size) => {
                ctx.total_bytes.fetch_add(size, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::error!(
                    "[metaanalysis] attachment '{}' failed: {}",
                    file.name,
                    e
                );
            }
        }
    }
}

fn file_exists_nonempty(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

/// Download a Bunny Stream video: resolve the embed to its CDN host, then pull
/// the source MP4 (`/original`), falling back to the HLS playlist.
async fn download_bunny_video(
    client: &reqwest::Client,
    embed_url: &str,
    output_path: &str,
    cancel_token: &CancellationToken,
) -> anyhow::Result<u64> {
    let media = api::resolve_bunny(client, embed_url).await?;

    if let Some(ref original) = media.original_url {
        match download_file_direct(client, original, output_path, cancel_token).await {
            Ok(size) if size > 0 => return Ok(size),
            Ok(_) => {
                let _ = std::fs::remove_file(output_path);
            }
            Err(e) => {
                tracing::warn!(
                    "[metaanalysis] /original failed ({}), trying HLS",
                    e
                );
                let _ = std::fs::remove_file(output_path);
            }
        }
    }

    if let Some(ref hls) = media.hls_url {
        return download_hls_video(hls, output_path, cancel_token).await;
    }

    Err(anyhow!("no downloadable Bunny stream for {}", embed_url))
}

async fn download_hls_video(
    m3u8_url: &str,
    output_path: &str,
    cancel_token: &CancellationToken,
) -> anyhow::Result<u64> {
    use omniget_core::core::media_processor::MediaProcessor;

    MediaProcessor::download_hls(
        m3u8_url,
        output_path,
        "https://iframe.mediadelivery.net/",
        None,
        cancel_token.clone(),
        20,
        3,
        None,
    )
    .await?;

    let meta = std::fs::metadata(output_path)?;
    Ok(meta.len())
}

async fn download_file_direct(
    client: &reqwest::Client,
    url: &str,
    output_path: &str,
    cancel_token: &CancellationToken,
) -> anyhow::Result<u64> {
    let out = Path::new(output_path);
    let (tx, _rx) = mpsc::channel(8);
    omniget_core::core::direct_downloader::download_direct(
        client,
        url,
        out,
        tx,
        Some(cancel_token),
    )
    .await
}

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use futures::StreamExt;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use omniget_core::core::media_processor::MediaProcessor;

use super::api::{self, UdemyCourse};
use super::auth::UdemySession;
use super::vtt_to_srt::vtt_to_srt;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 Edg/145.0.0.0";
const MAX_FILENAME_BYTES: usize = 200;

#[derive(Debug, Clone, serde::Serialize)]
pub struct UdemyCourseDownloadProgress {
    pub course_id: u64,
    pub course_name: String,
    pub percent: f64,
    pub current_chapter: String,
    pub current_lecture: String,
    pub downloaded_bytes: u64,
    pub total_lectures: u32,
    pub completed_lectures: u32,
}

pub struct UdemyDownloader {
    session: Arc<Mutex<Option<UdemySession>>>,
    hls_client: reqwest::Client,
    max_concurrent_segments: u32,
    max_retries: u32,
    keep_vtt: bool,
    target_quality: Option<u32>,
    continuous_lecture_numbers: bool,
    chapter_filter: HashSet<u32>,
    download_captions: bool,
    caption_locale: String,
    course_locale: Option<String>,
}

struct CaptionTrack {
    url: String,
    locale_id: String,
    locale_prefix: String,
    display_lang: String,
    auto_generated: bool,
}

impl CaptionTrack {
    fn canonical(&self) -> &str {
        if !self.locale_id.is_empty() {
            &self.locale_id
        } else if !self.display_lang.is_empty() {
            &self.display_lang
        } else {
            "unknown"
        }
    }

    fn matches(&self, key: &str) -> bool {
        !key.is_empty()
            && (self.locale_id.to_lowercase() == key
                || self.locale_prefix == key
                || self.display_lang.to_lowercase() == key)
    }
}

fn strip_emojis(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            !((0x1F300..=0x1FAFF).contains(&cp)
                || (0x2600..=0x27BF).contains(&cp)
                || (0x1F000..=0x1F2FF).contains(&cp)
                || (0x2300..=0x23FF).contains(&cp)
                || (0xFE00..=0xFE0F).contains(&cp)
                || (0x1F1E6..=0x1F1FF).contains(&cp)
                || cp == 0x200D
                || cp == 0x20E3)
        })
        .collect()
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let is_space = c.is_whitespace();
        if is_space {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn safe_filename(name: &str) -> String {
    let stripped = strip_emojis(name);
    let collapsed = collapse_whitespace(&stripped);
    let sanitized = sanitize_filename::sanitize(&collapsed);
    if sanitized.is_empty() {
        return "untitled".into();
    }
    if sanitized.len() <= MAX_FILENAME_BYTES {
        return sanitized;
    }
    let mut end = MAX_FILENAME_BYTES;
    while !sanitized.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    sanitized[..end].trim_end().to_string()
}

pub(crate) fn parse_quality_pref(s: &str) -> Option<u32> {
    let trimmed = s.trim().to_lowercase();
    if trimmed == "best" || trimmed.is_empty() {
        return None;
    }
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

impl UdemyDownloader {
    pub fn new(
        session: Arc<Mutex<Option<UdemySession>>>,
        max_concurrent_segments: u32,
        max_retries: u32,
        keep_vtt: bool,
        target_quality: Option<u32>,
        continuous_lecture_numbers: bool,
        chapter_filter: HashSet<u32>,
        download_captions: bool,
        caption_locale: String,
        course_locale: Option<String>,
    ) -> Self {
        let hls_client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            session,
            hls_client,
            max_concurrent_segments,
            max_retries,
            keep_vtt,
            target_quality,
            continuous_lecture_numbers,
            chapter_filter,
            download_captions,
            caption_locale,
            course_locale,
        }
    }

    pub async fn download_full_course(
        &self,
        course: &UdemyCourse,
        output_dir: &str,
        curriculum: api::UdemyCurriculum,
        progress_tx: mpsc::Sender<UdemyCourseDownloadProgress>,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<u32> {
        let session = {
            let guard = self.session.lock().await;
            guard.clone().ok_or_else(|| anyhow!("Not authenticated"))?
        };

        let _ = progress_tx.send(UdemyCourseDownloadProgress {
            course_id: course.id,
            course_name: course.title.clone(),
            percent: 0.0,
            current_chapter: String::new(),
            current_lecture: String::new(),
            downloaded_bytes: 0,
            total_lectures: curriculum.total_lectures,
            completed_lectures: 0,
        }).await;

        let course_dir_name = safe_filename(&course.title);
        let course_dir = PathBuf::from(output_dir).join(&course_dir_name);
        std::fs::create_dir_all(&course_dir)?;

        let total_lectures: u32 = if self.chapter_filter.is_empty() {
            curriculum.total_lectures
        } else {
            curriculum
                .chapters
                .iter()
                .enumerate()
                .filter(|(idx, _)| self.chapter_filter.contains(&((*idx + 1) as u32)))
                .map(|(_, ch)| ch.lectures.len() as u32)
                .sum()
        };
        let mut completed_lectures: u32 = 0;
        let mut downloaded_bytes: u64 = 0;
        let mut drm_skipped: u32 = 0;
        let mut global_lecture_counter: u32 = 0;

        for (ch_idx, chapter) in curriculum.chapters.iter().enumerate() {
            if cancel_token.is_cancelled() {
                return Err(anyhow!("Download cancelled"));
            }

            let chapter_number = (ch_idx + 1) as u32;
            if !self.chapter_filter.is_empty() && !self.chapter_filter.contains(&chapter_number) {
                tracing::info!(
                    "[udemy] skipping chapter {} '{}' (not in filter)",
                    chapter_number, chapter.title
                );
                continue;
            }

            let chapter_dir_name = format!(
                "{:02} - {}",
                chapter_number,
                safe_filename(&chapter.title)
            );
            let chapter_dir = course_dir.join(&chapter_dir_name);
            std::fs::create_dir_all(&chapter_dir)?;

            for (lec_idx, lecture) in chapter.lectures.iter().enumerate() {
                if cancel_token.is_cancelled() {
                    return Err(anyhow!("Download cancelled"));
                }

                let lecture_num = if self.continuous_lecture_numbers {
                    global_lecture_counter += 1;
                    global_lecture_counter
                } else {
                    (lec_idx + 1) as u32
                };

                let _ = progress_tx.send(UdemyCourseDownloadProgress {
                    course_id: course.id,
                    course_name: course.title.clone(),
                    percent: if total_lectures > 0 {
                        (completed_lectures as f64 / total_lectures as f64) * 100.0
                    } else {
                        0.0
                    },
                    current_chapter: chapter.title.clone(),
                    current_lecture: format!("{}/{} - {}", lecture_num, chapter.lectures.len(), lecture.title),
                    downloaded_bytes,
                    total_lectures,
                    completed_lectures,
                }).await;

                let result = self.download_lecture(
                    &session,
                    course.id,
                    lecture,
                    &chapter_dir,
                    &cancel_token,
                    lecture_num,
                ).await;

                match result {
                    Ok((b, drm)) => {
                        downloaded_bytes += b;
                        drm_skipped += drm;
                    }
                    Err(e) => {
                        tracing::error!(
                            "[udemy] failed to download lecture '{}': {}",
                            lecture.title, e
                        );
                    }
                }

                if lecture.lecture_class == "lecture" {
                    completed_lectures += 1;
                }
            }
        }

        let _ = progress_tx.send(UdemyCourseDownloadProgress {
            course_id: course.id,
            course_name: course.title.clone(),
            percent: 100.0,
            current_chapter: String::new(),
            current_lecture: String::new(),
            downloaded_bytes,
            total_lectures,
            completed_lectures,
        }).await;

        tracing::info!(
            "[udemy] course '{}' download complete: {} lectures, {} bytes, {} drm-skipped",
            course.title, completed_lectures, downloaded_bytes, drm_skipped
        );

        Ok(drm_skipped)
    }

    async fn download_lecture(
        &self,
        session: &UdemySession,
        course_id: u64,
        lecture: &api::UdemyLecture,
        chapter_dir: &Path,
        cancel_token: &CancellationToken,
        lecture_num: u32,
    ) -> anyhow::Result<(u64, u32)> {
        if lecture.lecture_class == "quiz" || lecture.lecture_class == "practice" {
            let quiz_file = chapter_dir.join(format!("{:02} - {} [quiz].json", lecture_num, safe_filename(&lecture.title)));
            if !file_exists_with_content(&quiz_file) {
                if let Some(asset) = &lecture.asset {
                    let json_str = serde_json::to_string_pretty(asset).unwrap_or_default();
                    if !json_str.is_empty() {
                        let _ = std::fs::write(&quiz_file, json_str);
                    }
                }
            }
            return Ok((0, 0));
        }

        let asset = match &lecture.asset {
            Some(a) => a,
            None => return Ok((0, 0)),
        };

        let asset_type = asset.get("asset_type")
            .or_else(|| asset.get("assetType"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let mut total_bytes: u64 = 0;
        let mut drm_skipped: u32 = 0;

        match asset_type.as_str() {
            "video" => {
                let (bytes, drm) = self.download_video_asset(
                    session, course_id, lecture.id, asset, &lecture.title, chapter_dir, cancel_token, lecture_num
                ).await?;
                total_bytes += bytes;
                drm_skipped += drm;
            }
            "article" => {
                let body = asset.get("body").and_then(|v| v.as_str()).unwrap_or("");
                if !body.is_empty() {
                    let file_name = format!(
                        "{:02} - {}.html",
                        lecture_num,
                        safe_filename(&lecture.title)
                    );
                    let file_path = chapter_dir.join(&file_name);
                    if !file_exists_with_content(&file_path) {
                        std::fs::write(&file_path, body.as_bytes())?;
                        tracing::info!("[udemy] saved article: {}", file_name);
                    }
                }
            }
            "file" | "e-book" | "presentation" | "audio" => {
                total_bytes += self.download_downloadable_asset(
                    session, asset, &lecture.title, chapter_dir, lecture_num
                ).await?;
            }
            _ => {
                if !asset_type.is_empty() {
                    tracing::warn!("[udemy] unknown asset type '{}' for '{}'", asset_type, lecture.title);
                }
            }
        }

        if self.download_captions {
            if let Some(tracks) = asset.get("captions").and_then(|v| v.as_array()) {
                total_bytes += self.download_lecture_captions(
                    session, tracks, &lecture.title, chapter_dir, lecture_num
                ).await;
            }
        }

        let supp_assets = asset.get("supplementary_assets").and_then(|v| v.as_array());
        if let Some(assets) = supp_assets {
            for supp in assets {
                total_bytes += self.download_supplementary_asset(
                    session, supp, chapter_dir, lecture_num
                ).await.unwrap_or(0);
            }
        }

        Ok((total_bytes, drm_skipped))
    }

    async fn download_lecture_captions(
        &self,
        session: &UdemySession,
        tracks: &[serde_json::Value],
        lecture_title: &str,
        chapter_dir: &Path,
        lecture_num: u32,
    ) -> u64 {
        let mut candidates: Vec<CaptionTrack> = Vec::new();

        for track in tracks {
            let class = track.get("_class").and_then(|v| v.as_str()).unwrap_or("");
            if class != "caption" {
                continue;
            }
            let url = match track.get("url").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => continue,
            };

            let locale_id = track
                .get("locale_id")
                .or_else(|| track.get("localeId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let locale_prefix = locale_id
                .to_lowercase()
                .split('_')
                .next()
                .unwrap_or("")
                .to_string();

            let display_lang = track
                .get("language")
                .or_else(|| track.get("srclang"))
                .or_else(|| track.get("label"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let auto_generated = track
                .get("source")
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("auto"))
                .unwrap_or(false)
                || track
                    .get("video_label")
                    .or_else(|| track.get("title"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase().contains("auto"))
                    .unwrap_or(false);

            candidates.push(CaptionTrack {
                url,
                locale_id,
                locale_prefix,
                display_lang,
                auto_generated,
            });
        }

        if candidates.is_empty() {
            return 0;
        }

        let wanted = self.caption_locale.trim().to_lowercase();

        if wanted == "all" {
            let mut bytes: u64 = 0;
            for track in &candidates {
                bytes += self.download_caption_track(
                    session, &track.url, track.canonical(), lecture_title, chapter_dir, lecture_num
                ).await;
            }
            return bytes;
        }

        let pick = |key: &str| -> Option<usize> {
            let group: Vec<usize> = candidates
                .iter()
                .enumerate()
                .filter(|(_, t)| t.matches(key))
                .map(|(i, _)| i)
                .collect();
            group
                .iter()
                .copied()
                .find(|&i| !candidates[i].auto_generated)
                .or_else(|| group.first().copied())
        };

        let course_locale_lc = self
            .course_locale
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let course_prefix = course_locale_lc
            .split('_')
            .next()
            .unwrap_or("")
            .to_string();

        let mut chosen: Option<usize> = None;
        if !wanted.is_empty() {
            chosen = pick(&wanted);
            if chosen.is_none() {
                let available: Vec<&str> = candidates.iter().map(|t| t.canonical()).collect();
                tracing::warn!(
                    "[udemy] no caption matched requested locale '{}' for lecture '{}'. Available: {:?}; falling back to course locale/English",
                    self.caption_locale,
                    lecture_title,
                    available
                );
            }
        }
        if chosen.is_none() && !course_locale_lc.is_empty() {
            chosen = pick(&course_locale_lc).or_else(|| pick(&course_prefix));
        }
        if chosen.is_none() {
            chosen = pick("en");
        }
        if chosen.is_none() {
            chosen = candidates
                .iter()
                .position(|t| !t.auto_generated)
                .or(if candidates.is_empty() { None } else { Some(0) });
        }

        let track = match chosen {
            Some(i) => &candidates[i],
            None => return 0,
        };

        tracing::info!(
            "[udemy] caption selected: locale='{}' auto_generated={} (requested='{}', course_locale='{}') for '{}'",
            track.canonical(),
            track.auto_generated,
            self.caption_locale,
            self.course_locale.as_deref().unwrap_or("unknown"),
            lecture_title
        );

        self.download_caption_track(
            session, &track.url, track.canonical(), lecture_title, chapter_dir, lecture_num
        ).await
    }

    async fn download_caption_track(
        &self,
        session: &UdemySession,
        url: &str,
        canonical: &str,
        lecture_title: &str,
        chapter_dir: &Path,
        lecture_num: u32,
    ) -> u64 {
        let canonical_safe = safe_filename(canonical);
        let ext = if url.contains(".vtt") { "vtt" } else { "srt" };
        let caption_name = format!(
            "{:02} - {}.{}.{}",
            lecture_num,
            safe_filename(lecture_title),
            canonical_safe,
            ext
        );
        let caption_path = chapter_dir.join(&caption_name);

        let mut bytes: u64 = 0;
        if !file_exists_with_content(&caption_path) {
            match download_file_simple(&session.client, url, &caption_path).await {
                Ok(b) => {
                    bytes = b;
                    tracing::info!("[udemy] saved caption: {}", caption_name);
                }
                Err(e) => {
                    tracing::warn!("[udemy] failed to download caption '{}': {}", caption_name, e);
                }
            }
        }

        if ext == "vtt" && file_exists_with_content(&caption_path) {
            let srt_path = caption_path.with_extension("srt");
            if !file_exists_with_content(&srt_path) {
                match vtt_to_srt(&caption_path, &srt_path) {
                    Ok(()) => {
                        tracing::info!("[udemy] converted caption to srt: {}", srt_path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                        if !self.keep_vtt {
                            let _ = std::fs::remove_file(&caption_path);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[udemy] vtt→srt conversion failed for '{}': {}", caption_name, e);
                    }
                }
            }
        }

        bytes
    }

    async fn download_video_asset(
        &self,
        session: &UdemySession,
        course_id: u64,
        lecture_id: u64,
        asset: &serde_json::Value,
        title: &str,
        chapter_dir: &Path,
        cancel_token: &CancellationToken,
        lecture_num: u32,
    ) -> anyhow::Result<(u64, u32)> {
        let file_name = format!("{:02} - {}.mp4", lecture_num, safe_filename(title));
        let file_path = chapter_dir.join(&file_name);

        if file_exists_with_content(&file_path) {
            tracing::info!("[udemy] skipping existing video: {}", file_name);
            return Ok((0, 0));
        }

        let refreshed = match api::get_fresh_lecture_asset(
            session, &session.portal_name, course_id, lecture_id
        ).await {
            Ok(fresh) => Some(fresh),
            Err(e) => {
                tracing::warn!(
                    "[udemy] could not refresh media URLs for '{}', using curriculum data: {}",
                    title, e
                );
                None
            }
        };
        let asset = refreshed.as_ref().unwrap_or(asset);

        let has_license_token = asset.get("media_license_token")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        let drm_signaled = has_license_token
            || asset.get("course_is_drmed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

        let stream_urls = asset.get("stream_urls");
        if let Some(streams) = stream_urls {
            if let Some(videos) = streams.get("Video").and_then(|v| v.as_array()) {
                match self.download_from_stream_urls(
                    session, videos, &file_path, title, cancel_token
                ).await {
                    Ok(bytes) => return Ok((bytes, 0)),
                    Err(e) => return self.map_video_download_error(e, title, drm_signaled),
                }
            }
        }

        if let Some(sources) = asset.get("media_sources").and_then(|v| v.as_array()) {
            let result = self.download_from_media_sources(
                session, sources, &file_path, title, cancel_token
            ).await;

            match result {
                Ok(bytes) if bytes > 0 => return Ok((bytes, 0)),
                Ok(_) => {}
                Err(e) => return self.map_video_download_error(e, title, drm_signaled),
            }

            if drm_signaled {
                tracing::warn!(
                    "[udemy] DRM-protected video skipped: '{}' (no downloadable sources available)",
                    title
                );
                return Ok((0, 1));
            }

            tracing::warn!(
                "[udemy] no usable video sources in media_sources for '{}' (types: {})",
                title,
                sources.iter()
                    .filter_map(|s| s.get("type").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return Ok((0, 0));
        }

        tracing::warn!("[udemy] no video sources found for '{}'", title);
        Ok((0, 0))
    }

    fn map_video_download_error(
        &self,
        e: anyhow::Error,
        title: &str,
        drm_signaled: bool,
    ) -> anyhow::Result<(u64, u32)> {
        let msg = e.to_string();

        if msg.contains("SAMPLE-AES") {
            tracing::warn!("[udemy] DRM-protected (SAMPLE-AES) lecture skipped: '{}'", title);
            return Ok((0, 1));
        }

        if msg.contains("403") {
            if drm_signaled {
                tracing::warn!("[udemy] DRM-protected lecture skipped (403 on protected media): '{}'", title);
                return Ok((0, 1));
            }
            return Err(anyhow!(
                "Access denied (403) downloading '{}': the media URL was rejected even after being refreshed — the account may lack access to this lecture or it is region-restricted (not DRM)",
                title
            ));
        }

        if drm_signaled {
            tracing::warn!("[udemy] DRM-protected lecture skipped: '{}' ({})", title, msg);
            return Ok((0, 1));
        }

        Err(e)
    }

    async fn download_from_stream_urls(
        &self,
        session: &UdemySession,
        videos: &[serde_json::Value],
        file_path: &Path,
        title: &str,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<u64> {
        let mut sources: Vec<(&str, u32)> = Vec::new();
        for v in videos {
            let label = v.get("label").and_then(|l| l.as_str()).unwrap_or("0");
            let url = match v.get("file").and_then(|f| f.as_str()) {
                Some(u) => u,
                None => continue,
            };
            if label.to_lowercase() == "audio" {
                continue;
            }
            let height: u32 = label.parse().unwrap_or(0);
            sources.push((url, height));
        }

        if sources.is_empty() {
            tracing::warn!("[udemy] no valid video sources for '{}'", title);
            return Ok(0);
        }

        let (best_url, best_height) = if let Some(target) = self.target_quality {
            let chosen = sources
                .iter()
                .min_by_key(|(_, h)| (*h as i32 - target as i32).abs())
                .copied()
                .unwrap_or(sources[0]);
            if (chosen.1 as i32 - target as i32).abs() > 200 {
                tracing::info!(
                    "[udemy] requested {}p, using closest available {}p for '{}'",
                    target, chosen.1, title
                );
            }
            chosen
        } else {
            sources.sort_by(|a, b| b.1.cmp(&a.1));
            sources[0]
        };

        tracing::info!("[udemy] downloading '{}' at {}p", title, best_height);

        let url_str = best_url.to_string();
        let is_hls = url_str.contains(".m3u8");

        if is_hls {
            self.download_hls_and_remux(&url_str, file_path, title, cancel_token).await
        } else {
            let bytes = download_file_simple(&session.client, &url_str, file_path).await?;
            tracing::info!("[udemy] direct download complete: {} ({}p, {} bytes)", title, best_height, bytes);
            Ok(bytes)
        }
    }

    async fn download_from_media_sources(
        &self,
        session: &UdemySession,
        sources: &[serde_json::Value],
        file_path: &Path,
        title: &str,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<u64> {
        let mut mp4_sources: Vec<(&str, u32)> = Vec::new();
        let mut hls_source: Option<&str> = None;

        for source in sources {
            let src = match source.get("src").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let media_type = source.get("type").and_then(|v| v.as_str()).unwrap_or("");

            match media_type {
                "video/mp4" => {
                    let label = source.get("label").and_then(|v| v.as_str()).unwrap_or("0");
                    let height: u32 = label.parse().unwrap_or(0);
                    mp4_sources.push((src, height));
                }
                "application/x-mpegURL" => {
                    if hls_source.is_none() {
                        hls_source = Some(src);
                    }
                }
                _ => {}
            }
        }

        if !mp4_sources.is_empty() {
            let (best_url, best_height) = if let Some(target) = self.target_quality {
                let chosen = mp4_sources
                    .iter()
                    .min_by_key(|(_, h)| (*h as i32 - target as i32).abs())
                    .copied()
                    .unwrap_or(mp4_sources[0]);
                if (chosen.1 as i32 - target as i32).abs() > 200 {
                    tracing::info!(
                        "[udemy] requested {}p, using closest available {}p for '{}'",
                        target, chosen.1, title
                    );
                }
                chosen
            } else {
                mp4_sources.sort_by(|a, b| b.1.cmp(&a.1));
                mp4_sources[0]
            };

            tracing::info!("[udemy] downloading '{}' at {}p (via media_sources mp4)", title, best_height);

            let bytes = download_file_simple(&session.client, best_url, file_path).await?;
            tracing::info!("[udemy] direct download complete: {} ({}p, {} bytes)", title, best_height, bytes);
            return Ok(bytes);
        }

        if let Some(hls_url) = hls_source {
            tracing::info!("[udemy] downloading '{}' via HLS (media_sources)", title);
            return self.download_hls_and_remux(hls_url, file_path, title, cancel_token).await;
        }

        Ok(0)
    }

    async fn download_hls_and_remux(
        &self,
        hls_url: &str,
        file_path: &Path,
        title: &str,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<u64> {
        let temp_ts_path = file_path.with_extension("ts");
        let output_str_ts = temp_ts_path.to_string_lossy().to_string();
        let output_str_mp4 = file_path.to_string_lossy().to_string();

        let result = MediaProcessor::download_hls(
            hls_url,
            &output_str_ts,
            "https://www.udemy.com/",
            None,
            cancel_token.clone(),
            self.max_concurrent_segments,
            self.max_retries,
            Some(self.hls_client.clone()),
        ).await;

        match result {
            Ok(r) => {
                tracing::info!("[udemy] HLS download complete ({} bytes), remuxing...", r.file_size);
                if let Err(e) = MediaProcessor::remux(&output_str_ts, &output_str_mp4).await {
                    tracing::error!("[udemy] Remuxing failed: {}", e);
                    let _ = std::fs::remove_file(&temp_ts_path);
                    return Err(e);
                }
                let _ = std::fs::remove_file(&temp_ts_path);
                let final_size = std::fs::metadata(file_path)?.len();
                tracing::info!("[udemy] Remux complete: {}", title);
                Ok(final_size)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temp_ts_path);
                Err(e)
            }
        }
    }

    async fn download_downloadable_asset(
        &self,
        session: &UdemySession,
        asset: &serde_json::Value,
        title: &str,
        chapter_dir: &Path,
        lecture_num: u32,
    ) -> anyhow::Result<u64> {
        let filename = asset.get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let download_url = asset.get("download_urls")
            .and_then(|d| {
                if let Some(obj) = d.as_object() {
                    for (_key, val) in obj {
                        if let Some(arr) = val.as_array() {
                            if let Some(first) = arr.first() {
                                if let Some(url) = first.get("file").and_then(|f| f.as_str()) {
                                    return Some(url.to_string());
                                }
                            }
                        }
                    }
                }
                None
            });

        let url = match download_url {
            Some(u) => u,
            None => {
                tracing::warn!("[udemy] no download URL for asset '{}'", title);
                return Ok(0);
            }
        };

        let safe_name = if filename.is_empty() {
            format!("{:02} - {}.bin", lecture_num, safe_filename(title))
        } else {
            format!("{:02} - {}", lecture_num, safe_filename(filename))
        };

        let file_path = chapter_dir.join(&safe_name);

        if file_exists_with_content(&file_path) {
            tracing::info!("[udemy] skipping existing file: {}", safe_name);
            return Ok(0);
        }

        match download_file_simple(&session.client, &url, &file_path).await {
            Ok(b) => {
                tracing::info!("[udemy] downloaded asset: {}", safe_name);
                Ok(b)
            }
            Err(e) => {
                tracing::error!("[udemy] failed to download '{}': {}", safe_name, e);
                Ok(0)
            }
        }
    }

    async fn download_supplementary_asset(
        &self,
        session: &UdemySession,
        supp: &serde_json::Value,
        chapter_dir: &Path,
        lecture_num: u32,
    ) -> anyhow::Result<u64> {
        let asset_type = supp.get("asset_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let filename = supp.get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let title = supp.get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match asset_type.as_str() {
            "file" | "sourcecode" => {
                let download_url = supp.get("download_urls")
                    .and_then(|d| {
                        if let Some(obj) = d.as_object() {
                            for (_key, val) in obj {
                                if let Some(arr) = val.as_array() {
                                    if let Some(first) = arr.first() {
                                        return first.get("file").and_then(|f| f.as_str()).map(|s| s.to_string());
                                    }
                                }
                            }
                        }
                        None
                    });

                let url = match download_url {
                    Some(u) => u,
                    None => return Ok(0),
                };

                let base = if filename.is_empty() {
                    safe_filename(title)
                } else {
                    safe_filename(filename)
                };
                let safe_name = format!("{:02} - {}", lecture_num, base);
                let file_path = chapter_dir.join(&safe_name);

                if file_exists_with_content(&file_path) {
                    return Ok(0);
                }

                download_file_simple(&session.client, &url, &file_path).await.ok();
            }
            "externallink" => {
                let external_url = supp.get("external_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !external_url.is_empty() {
                    let base = if filename.is_empty() {
                        safe_filename(title)
                    } else {
                        safe_filename(filename)
                    };
                    let safe_name = format!("{:02} - {}.url", lecture_num, base);
                    let file_path = chapter_dir.join(&safe_name);
                    if !file_exists_with_content(&file_path) {
                        let content = format!("[InternetShortcut]\nURL={}", external_url);
                        std::fs::write(&file_path, content.as_bytes())?;
                    }
                }
            }
            _ => {}
        }

        Ok(0)
    }
}

fn file_exists_with_content(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => m.len() > 0,
        Err(_) => false,
    }
}

async fn download_file_simple(
    client: &reqwest::Client,
    url: &str,
    output_path: &Path,
) -> anyhow::Result<u64> {
    let part_path = output_path.with_extension(
        format!(
            "{}.part",
            output_path.extension().unwrap_or_default().to_string_lossy()
        )
    );

    let result = download_file_inner(client, url, output_path, &part_path).await;
    if result.is_err() {
        let _ = std::fs::remove_file(&part_path);
    }
    result
}

async fn download_file_inner(
    client: &reqwest::Client,
    url: &str,
    output_path: &Path,
    part_path: &Path,
) -> anyhow::Result<u64> {
    let resp = client.get(url)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| anyhow!("Download request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow!("Access denied (403): the signed media URL was rejected (expired signature or restricted access)"));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("Auth error (401), session may have expired"));
        }
        return Err(anyhow!("Download returned status {}", status));
    }

    let mut total: u64 = 0;
    let mut stream = resp.bytes_stream();
    let mut file = std::fs::File::create(part_path)?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("Stream error: {}", e))?;
        std::io::Write::write_all(&mut file, &chunk)?;
        total += chunk.len() as u64;
    }

    std::io::Write::flush(&mut file)?;
    drop(file);

    std::fs::rename(part_path, output_path)?;

    Ok(total)
}

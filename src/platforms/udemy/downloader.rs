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

const MAX_REPORTED_TITLES: usize = 50;
const ZERO_TAIL_PROBE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct UdemyDownloadSummary {
    pub downloaded_bytes: u64,
    pub lectures_processed: u32,
    pub videos_downloaded: u32,
    pub videos_already_present: u32,
    pub drm_skipped: u32,
    pub no_media: u32,
    pub failed: u32,
    pub drm_skipped_titles: Vec<String>,
    pub no_media_titles: Vec<String>,
    pub failed_titles: Vec<String>,
}

impl UdemyDownloadSummary {
    fn record(&mut self, title: &str, outcome: &VideoOutcome) {
        match outcome {
            VideoOutcome::NotVideo => {}
            VideoOutcome::Downloaded => self.videos_downloaded += 1,
            VideoOutcome::AlreadyPresent => self.videos_already_present += 1,
            VideoOutcome::Drm => {
                self.drm_skipped += 1;
                push_capped(&mut self.drm_skipped_titles, title.to_string());
            }
            VideoOutcome::NoMedia(reason) => {
                self.no_media += 1;
                push_capped(&mut self.no_media_titles, format!("{}: {}", title, reason));
            }
            VideoOutcome::Failed(reason) => {
                self.failed += 1;
                push_capped(&mut self.failed_titles, format!("{}: {}", title, reason));
            }
        }
    }
}

fn push_capped(list: &mut Vec<String>, item: String) {
    if list.len() < MAX_REPORTED_TITLES {
        list.push(item);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum VideoOutcome {
    NotVideo,
    Downloaded,
    AlreadyPresent,
    Drm,
    NoMedia(String),
    Failed(String),
}

struct LectureOutcome {
    bytes: u64,
    video: VideoOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoSource {
    pub url: String,
    pub height: u32,
    pub hls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ResumePlan {
    Fresh,
    Append { offset: u64 },
    AlreadyComplete,
}

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
    section_ids: HashSet<u64>,
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
        section_ids: HashSet<u64>,
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
            section_ids,
            download_captions,
            caption_locale,
            course_locale,
        }
    }

    fn chapter_selected(&self, index: u32, chapter_id: u64) -> bool {
        api::chapter_selected(index, chapter_id, &self.chapter_filter, &self.section_ids)
    }

    pub async fn download_full_course(
        &self,
        course: &UdemyCourse,
        output_dir: &str,
        curriculum: api::UdemyCurriculum,
        progress_tx: mpsc::Sender<UdemyCourseDownloadProgress>,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<UdemyDownloadSummary> {
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

        let total_lectures: u32 = curriculum
            .chapters
            .iter()
            .enumerate()
            .filter(|(idx, ch)| self.chapter_selected((*idx + 1) as u32, ch.id))
            .map(|(_, ch)| ch.lectures.iter().filter(|l| l.lecture_class == "lecture").count() as u32)
            .sum();
        let mut completed_lectures: u32 = 0;
        let mut downloaded_bytes: u64 = 0;
        let mut summary = UdemyDownloadSummary::default();
        let mut global_lecture_counter: u32 = 0;

        for (ch_idx, chapter) in curriculum.chapters.iter().enumerate() {
            if cancel_token.is_cancelled() {
                return Err(anyhow!("Download cancelled"));
            }

            let chapter_number = (ch_idx + 1) as u32;
            if !self.chapter_selected(chapter_number, chapter.id) {
                tracing::info!(
                    "[udemy] skipping chapter {} '{}' (id {}, not selected)",
                    chapter_number, chapter.title, chapter.id
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
                    Ok(outcome) => {
                        downloaded_bytes += outcome.bytes;
                        summary.record(&lecture.title, &outcome.video);
                    }
                    Err(e) => {
                        tracing::error!(
                            "[udemy] failed to download lecture '{}': {}",
                            lecture.title, e
                        );
                        summary.record(&lecture.title, &VideoOutcome::Failed(e.to_string()));
                    }
                }

                if lecture.lecture_class == "lecture" {
                    completed_lectures += 1;
                }
            }
        }

        tracing::info!("[udemy] downloading course-level resources for '{}'", course.title);
        match self.download_course_resources(&session, course.id, &course_dir).await {
            Ok(b) => {
                downloaded_bytes += b;
                tracing::info!("[udemy] resources downloaded: {} bytes", b);
            }
            Err(e) => {
                tracing::warn!("[udemy] resources download skipped: {}", e);
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

        summary.downloaded_bytes = downloaded_bytes;
        summary.lectures_processed = completed_lectures;

        tracing::info!(
            "[udemy] course '{}' done: {} lectures processed, {} videos downloaded, {} already present, {} drm, {} without media, {} failed, {} bytes",
            course.title,
            completed_lectures,
            summary.videos_downloaded,
            summary.videos_already_present,
            summary.drm_skipped,
            summary.no_media,
            summary.failed,
            downloaded_bytes
        );
        if !summary.no_media_titles.is_empty() {
            tracing::warn!("[udemy] lectures without media: {:?}", summary.no_media_titles);
        }
        if !summary.failed_titles.is_empty() {
            tracing::warn!("[udemy] lectures that failed: {:?}", summary.failed_titles);
        }

        Ok(summary)
    }

    async fn download_lecture(
        &self,
        session: &UdemySession,
        course_id: u64,
        lecture: &api::UdemyLecture,
        chapter_dir: &Path,
        cancel_token: &CancellationToken,
        lecture_num: u32,
    ) -> anyhow::Result<LectureOutcome> {
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
            return Ok(LectureOutcome { bytes: 0, video: VideoOutcome::NotVideo });
        }

        let asset = match &lecture.asset {
            Some(a) if !a.is_null() => a,
            _ => {
                tracing::warn!("[udemy] lecture '{}' has no asset in the curriculum", lecture.title);
                return Ok(LectureOutcome {
                    bytes: 0,
                    video: VideoOutcome::NoMedia("curriculum item has no asset".into()),
                });
            }
        };

        let asset_type = api::asset_type_of(asset);

        let mut total_bytes: u64 = 0;
        let mut video = VideoOutcome::NotVideo;

        match asset_type.as_str() {
            "video" => {
                let (bytes, outcome) = self.download_video_asset(
                    session, course_id, lecture.id, asset, &lecture.title, chapter_dir, cancel_token, lecture_num
                ).await;
                total_bytes += bytes;
                video = outcome;
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
                tracing::warn!("[udemy] unknown asset type '{}' for '{}'", asset_type, lecture.title);
                video = VideoOutcome::NoMedia(format!("unsupported asset type '{}'", asset_type));
            }
        }

        if self.download_captions {
            if let Some(tracks) = asset.get("captions").and_then(|v| v.as_array()) {
                total_bytes += self.download_lecture_captions(
                    session, tracks, &lecture.title, chapter_dir, lecture_num
                ).await;
            }
        }

        if let Some(assets) = &lecture.supplementary_assets {
            for supp in assets {
                total_bytes += self.download_supplementary_asset(
                    session, supp, chapter_dir, lecture_num
                ).await.unwrap_or(0);
            }
        }

        Ok(LectureOutcome { bytes: total_bytes, video })
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
    ) -> (u64, VideoOutcome) {
        let file_name = format!("{:02} - {}.mp4", lecture_num, safe_filename(title));
        let file_path = chapter_dir.join(&file_name);

        if file_exists_with_content(&file_path) {
            tracing::info!("[udemy] skipping existing video: {}", file_name);
            return (0, VideoOutcome::AlreadyPresent);
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

        let drm_signaled = asset.get("media_license_token")
            .map(|v| !v.is_null())
            .unwrap_or(false)
            || asset.get("course_is_drmed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

        let sources = collect_video_sources(asset);
        if sources.is_empty() {
            let types: Vec<String> = asset.get("media_sources")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|m| m.get("type").and_then(|t| t.as_str())).map(|t| t.to_string()).collect())
                .unwrap_or_default();
            if drm_signaled {
                tracing::warn!("[udemy] DRM-protected video skipped: '{}' (media types: {:?})", title, types);
                return (0, VideoOutcome::Drm);
            }
            let keys: Vec<&str> = asset.as_object().map(|o| o.keys().map(|k| k.as_str()).collect()).unwrap_or_default();
            tracing::warn!(
                "[udemy] no video sources for '{}' (refreshed: {}, media types: {:?}, asset keys: {:?})",
                title, refreshed.is_some(), types, keys
            );
            let reason = if types.is_empty() {
                format!("no stream_urls/media_sources in asset (fields: {})", keys.join(","))
            } else {
                format!("only non-downloadable media types ({})", types.join(","))
            };
            return (0, VideoOutcome::NoMedia(reason));
        }

        let direct = pick_direct(&sources, self.target_quality);
        let hls = sources.iter().find(|s| s.hls).cloned();
        let requested = self.target_quality
            .map(|t| format!("{}p", t))
            .unwrap_or_else(|| "best".to_string());

        let mut hls_plan: Option<VideoSource> = None;
        if let Some(hls_src) = hls {
            if prefer_hls(direct.as_ref().map(|d| d.height), self.target_quality) {
                match self.peek_hls_max_height(&hls_src.url).await {
                    Some(max_h) => {
                        let beats_direct = direct.as_ref().map(|d| max_h > d.height).unwrap_or(true);
                        if beats_direct {
                            tracing::info!(
                                "[udemy] '{}': requested {}, using HLS (up to {}p) over direct mp4 ({})",
                                title, requested, max_h,
                                direct.as_ref().map(|d| format!("{}p", d.height)).unwrap_or_else(|| "none".into())
                            );
                            hls_plan = Some(hls_src);
                        } else {
                            tracing::info!(
                                "[udemy] '{}': requested {}, HLS tops out at {}p, direct mp4 {}p is as good — using direct",
                                title, requested, max_h,
                                direct.as_ref().map(|d| d.height).unwrap_or(0)
                            );
                        }
                    }
                    None if direct.is_none() => hls_plan = Some(hls_src),
                    None => {
                        tracing::warn!(
                            "[udemy] '{}': could not read HLS master playlist, falling back to direct mp4",
                            title
                        );
                    }
                }
            }
        }

        if let Some(hls_src) = hls_plan {
            match self.download_hls_and_remux(&hls_src.url, &file_path, title, cancel_token).await {
                Ok(bytes) if bytes > 0 => return (bytes, VideoOutcome::Downloaded),
                Ok(_) => {}
                Err(e) => {
                    let outcome = map_video_download_error(&e, title, drm_signaled);
                    if outcome == VideoOutcome::Drm || direct.is_none() {
                        return (0, outcome);
                    }
                    tracing::warn!(
                        "[udemy] '{}': HLS download failed ({}), falling back to direct mp4",
                        title, e
                    );
                }
            }
        }

        let direct = match direct {
            Some(d) => d,
            None => {
                return (0, VideoOutcome::NoMedia("only an HLS stream is offered and it could not be downloaded".into()));
            }
        };

        if let Some(target) = self.target_quality {
            if direct.height != target {
                tracing::info!(
                    "[udemy] '{}': requested {}p, closest direct mp4 is {}p (available: {:?})",
                    title, target, direct.height,
                    sources.iter().filter(|s| !s.hls).map(|s| s.height).collect::<Vec<_>>()
                );
            }
        }
        tracing::info!("[udemy] downloading '{}' at {}p (direct mp4)", title, direct.height);

        match download_file_simple(&session.client, &direct.url, &file_path).await {
            Ok(bytes) => {
                tracing::info!("[udemy] direct download complete: {} ({}p, {} bytes)", title, direct.height, bytes);
                (bytes, VideoOutcome::Downloaded)
            }
            Err(e) => (0, map_video_download_error(&e, title, drm_signaled)),
        }
    }

    async fn peek_hls_max_height(&self, hls_url: &str) -> Option<u32> {
        let resp = self.hls_client
            .get(hls_url)
            .header("Referer", "https://www.udemy.com/")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            tracing::warn!("[udemy] HLS master playlist returned {}", resp.status());
            return None;
        }
        let text = resp.text().await.ok()?;
        hls_max_height(&text)
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

        let result = MediaProcessor::download_hls_with_quality(
            hls_url,
            &output_str_ts,
            "https://www.udemy.com/",
            None,
            cancel_token.clone(),
            self.max_concurrent_segments,
            self.max_retries,
            Some(self.hls_client.clone()),
            self.target_quality,
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

        if asset_type == "externallink" {
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
            return Ok(0);
        }

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

        match download_file_simple(&session.client, &url, &file_path).await {
            Ok(bytes) => Ok(bytes),
            Err(e) => {
                tracing::warn!(
                    "[udemy] failed to download supplementary asset '{}': {}",
                    safe_name,
                    e
                );
                Ok(0)
            }
        }
    }

    async fn download_course_resources(
        &self,
        session: &UdemySession,
        course_id: u64,
        course_dir: &Path,
    ) -> anyhow::Result<u64> {
        let resources = api::get_course_resources(
            session, &session.portal_name, course_id
        ).await?;

        if resources.is_empty() {
            return Ok(0);
        }

        let res_dir = course_dir.join("00 - Course Resources");
        std::fs::create_dir_all(&res_dir)?;

        let mut total_bytes: u64 = 0;

        for (i, res) in resources.iter().enumerate() {
            let title = res.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let filename = res.get("filename").and_then(|v| v.as_str()).unwrap_or("");

            let download_url = res.get("download_urls").and_then(|d| {
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

            if let Some(url) = download_url {
                let base = if filename.is_empty() {
                    safe_filename(title)
                } else {
                    safe_filename(filename)
                };
                let safe_name = format!("{:02} - {}", i + 1, base);
                let file_path = res_dir.join(&safe_name);

                if file_exists_with_content(&file_path) {
                    total_bytes += std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                    continue;
                }

                match download_file_simple(&session.client, &url, &file_path).await {
                    Ok(b) => {
                        total_bytes += b;
                        tracing::info!("[udemy] downloaded resource: {}", safe_name);
                    }
                    Err(e) => {
                        tracing::warn!("[udemy] failed to download resource '{}': {}", safe_name, e);
                    }
                }
            } else {
                let external_url = res.get("url")
                    .or_else(|| res.get("external_url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !external_url.is_empty() {
                    let base = if filename.is_empty() {
                        safe_filename(title)
                    } else {
                        safe_filename(filename)
                    };
                    let safe_name = format!("{:02} - {}.url", i + 1, base);
                    let file_path = res_dir.join(&safe_name);
                    if !file_exists_with_content(&file_path) {
                        let content = format!("[InternetShortcut]\nURL={}", external_url);
                        std::fs::write(&file_path, content.as_bytes())?;
                    }
                }
            }
        }

        Ok(total_bytes)
    }
}

fn file_exists_with_content(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => m.len() > 0,
        Err(_) => false,
    }
}

fn map_video_download_error(e: &anyhow::Error, title: &str, drm_signaled: bool) -> VideoOutcome {
    let msg = e.to_string();

    if msg.contains("SAMPLE-AES") {
        tracing::warn!("[udemy] DRM-protected (SAMPLE-AES) lecture skipped: '{}'", title);
        return VideoOutcome::Drm;
    }

    if msg.contains("403") {
        if drm_signaled {
            tracing::warn!("[udemy] DRM-protected lecture skipped (403 on protected media): '{}'", title);
            return VideoOutcome::Drm;
        }
        return VideoOutcome::Failed(format!(
            "Access denied (403) downloading '{}': the media URL was rejected even after being refreshed — the account may lack access to this lecture or it is region-restricted (not DRM)",
            title
        ));
    }

    if drm_signaled {
        tracing::warn!("[udemy] DRM-protected lecture skipped: '{}' ({})", title, msg);
        return VideoOutcome::Drm;
    }

    VideoOutcome::Failed(msg)
}

fn is_hls_entry(url: &str, media_type: &str) -> bool {
    media_type.eq_ignore_ascii_case("application/x-mpegURL")
        || media_type.eq_ignore_ascii_case("application/vnd.apple.mpegurl")
        || url.contains(".m3u8")
}

fn parse_height_label(label: &str) -> u32 {
    let digits: String = label.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

pub(crate) fn collect_video_sources(asset: &serde_json::Value) -> Vec<VideoSource> {
    let mut out: Vec<VideoSource> = Vec::new();
    let mut push = |url: &str, height: u32, hls: bool| {
        if url.is_empty() || out.iter().any(|s| s.url == url) {
            return;
        }
        out.push(VideoSource { url: url.to_string(), height, hls });
    };

    if let Some(videos) = asset.get("stream_urls").and_then(|s| s.get("Video")).and_then(|v| v.as_array()) {
        for v in videos {
            let url = v.get("file").and_then(|f| f.as_str()).unwrap_or("");
            let label = v.get("label").and_then(|l| l.as_str()).unwrap_or("");
            let media_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if label.eq_ignore_ascii_case("audio") {
                continue;
            }
            let hls = is_hls_entry(url, media_type);
            if !hls && !media_type.is_empty() && !media_type.eq_ignore_ascii_case("video/mp4") {
                continue;
            }
            push(url, if hls { 0 } else { parse_height_label(label) }, hls);
        }
    }

    if let Some(sources) = asset.get("media_sources").and_then(|v| v.as_array()) {
        for source in sources {
            let url = source.get("src").and_then(|v| v.as_str()).unwrap_or("");
            let media_type = source.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let label = source.get("label").and_then(|v| v.as_str()).unwrap_or("");
            if media_type.eq_ignore_ascii_case("video/mp4") {
                push(url, parse_height_label(label), false);
            } else if is_hls_entry(url, media_type) {
                push(url, 0, true);
            }
        }
    }

    out
}

pub(crate) fn pick_direct(sources: &[VideoSource], target: Option<u32>) -> Option<VideoSource> {
    let direct: Vec<&VideoSource> = sources.iter().filter(|s| !s.hls).collect();
    match target {
        Some(t) => direct
            .iter()
            .min_by_key(|s| ((s.height as i64 - t as i64).abs(), s.height < t))
            .map(|s| (*s).clone()),
        None => direct.iter().max_by_key(|s| s.height).map(|s| (*s).clone()),
    }
}

/// Udemy serves direct MP4 files up to 720p only; higher rungs live in the
/// HLS ladder, so HLS is worth the extra work whenever the request is not
/// already satisfied by a direct file.
pub(crate) fn prefer_hls(best_direct_height: Option<u32>, target: Option<u32>) -> bool {
    match (best_direct_height, target) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(direct), Some(t)) => t > direct,
    }
}

pub(crate) fn hls_max_height(master_text: &str) -> Option<u32> {
    let (_, master) = m3u8_rs::parse_master_playlist(master_text.as_bytes()).ok()?;
    master
        .variants
        .iter()
        .filter(|v| !v.is_i_frame)
        .filter_map(|v| v.resolution.as_ref().map(|r| r.height as u32))
        .max()
}

pub(crate) fn parse_content_range(header: &str) -> Option<(Option<u64>, Option<u64>)> {
    let rest = header.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let total = total.trim().parse::<u64>().ok();
    let start = if range.trim() == "*" {
        None
    } else {
        range.split_once('-').and_then(|(a, _)| a.trim().parse::<u64>().ok())
    };
    Some((start, total))
}

pub(crate) fn plan_resume(status: u16, existing: u64, content_range: Option<&str>) -> ResumePlan {
    if existing == 0 {
        return ResumePlan::Fresh;
    }
    match status {
        206 => match content_range.and_then(parse_content_range) {
            Some((Some(start), _)) if start == existing => ResumePlan::Append { offset: existing },
            Some((Some(start), _)) => {
                tracing::warn!("[udemy] server resumed at {} but part has {} bytes, restarting", start, existing);
                ResumePlan::Fresh
            }
            _ => ResumePlan::Append { offset: existing },
        },
        416 => match content_range.and_then(parse_content_range) {
            Some((_, Some(total))) if total == existing => ResumePlan::AlreadyComplete,
            _ => ResumePlan::Fresh,
        },
        _ => ResumePlan::Fresh,
    }
}

pub(crate) fn expected_total(
    plan: ResumePlan,
    content_length: Option<u64>,
    content_range: Option<&str>,
) -> Option<u64> {
    match plan {
        ResumePlan::Fresh => content_length,
        ResumePlan::Append { offset } => content_range
            .and_then(parse_content_range)
            .and_then(|(_, total)| total)
            .or_else(|| content_length.map(|len| offset + len)),
        ResumePlan::AlreadyComplete => None,
    }
}

/// After a power loss the file size may already be on disk while the last
/// data blocks never were, leaving a zero-filled tail; resuming after it
/// would splice zeros into the video.
pub(crate) fn zero_tail_len(tail: &[u8]) -> usize {
    tail.iter().rev().take_while(|b| **b == 0).count()
}

fn trim_zero_tail(part_path: &Path) -> anyhow::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};

    let len = std::fs::metadata(part_path)?.len();
    if len == 0 {
        return Ok(0);
    }
    let probe = len.min(ZERO_TAIL_PROBE_BYTES);
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(part_path)?;
    file.seek(SeekFrom::Start(len - probe))?;
    let mut buf = vec![0u8; probe as usize];
    file.read_exact(&mut buf)?;
    let zeros = zero_tail_len(&buf) as u64;
    if zeros == 0 {
        return Ok(len);
    }
    let keep = len - zeros;
    tracing::warn!(
        "[udemy] partial file {} has a {}-byte zero tail, truncating to {} before resuming",
        part_path.display(), zeros, keep
    );
    file.set_len(keep)?;
    Ok(keep)
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

    download_file_inner(client, url, output_path, &part_path).await
}

async fn download_file_inner(
    client: &reqwest::Client,
    url: &str,
    output_path: &Path,
    part_path: &Path,
) -> anyhow::Result<u64> {
    let existing = match std::fs::metadata(part_path) {
        Ok(m) if m.len() > 0 => trim_zero_tail(part_path).unwrap_or(0),
        _ => 0,
    };

    let mut req = client.get(url).timeout(Duration::from_secs(600));
    if existing > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={}-", existing));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("Download request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() && status != reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow!("Access denied (403): the signed media URL was rejected (expired signature or restricted access)"));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("Auth error (401), session may have expired"));
        }
        return Err(anyhow!("Download returned status {}", status));
    }

    let content_range = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let content_length = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let plan = plan_resume(status.as_u16(), existing, content_range.as_deref());
    let expected = expected_total(plan, content_length, content_range.as_deref());

    let (mut file, mut total) = match plan {
        ResumePlan::AlreadyComplete => {
            tracing::info!("[udemy] partial file already complete ({} bytes), finalizing", existing);
            std::fs::rename(part_path, output_path)?;
            return Ok(existing);
        }
        ResumePlan::Append { offset } => {
            tracing::info!("[udemy] resuming {} from byte {}", part_path.display(), offset);
            (std::fs::OpenOptions::new().append(true).open(part_path)?, offset)
        }
        ResumePlan::Fresh => {
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                let _ = std::fs::remove_file(part_path);
                return Err(anyhow!("Server rejected the resume range and the partial file was discarded; retry the download"));
            }
            if existing > 0 {
                tracing::info!("[udemy] server ignored the Range request, restarting {} from scratch", part_path.display());
            }
            (std::fs::File::create(part_path)?, 0)
        }
    };

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("Stream error after {} bytes (partial file kept for resume): {}", total, e))?;
        std::io::Write::write_all(&mut file, &chunk)?;
        total += chunk.len() as u64;
    }

    std::io::Write::flush(&mut file)?;
    file.sync_all()?;
    drop(file);

    if let Some(expected) = expected {
        if total < expected {
            return Err(anyhow!(
                "Download ended early: {} of {} bytes (partial file kept for resume)",
                total, expected
            ));
        }
        if total > expected {
            let _ = std::fs::remove_file(part_path);
            return Err(anyhow!(
                "Download produced {} bytes but the server announced {}; partial file discarded",
                total, expected
            ));
        }
    }

    std::fs::rename(part_path, output_path)?;

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn src(url: &str, height: u32, hls: bool) -> VideoSource {
        VideoSource { url: url.into(), height, hls }
    }

    #[test]
    fn collect_sources_merges_both_shapes_and_flags_hls() {
        let asset = json!({
            "stream_urls": {"Video": [
                {"label": "720", "file": "https://c/720.mp4", "type": "video/mp4"},
                {"label": "Auto", "file": "https://c/master.m3u8?x", "type": "application/x-mpegURL"},
                {"label": "audio", "file": "https://c/a.mp4"}
            ]},
            "media_sources": [
                {"label": "480", "src": "https://c/480.mp4", "type": "video/mp4"},
                {"label": "720", "src": "https://c/720.mp4", "type": "video/mp4"},
                {"label": "auto", "src": "https://c/dash.mpd", "type": "application/dash+xml"}
            ]
        });
        let sources = collect_video_sources(&asset);
        assert_eq!(sources, vec![
            src("https://c/720.mp4", 720, false),
            src("https://c/master.m3u8?x", 0, true),
            src("https://c/480.mp4", 480, false),
        ]);
    }

    #[test]
    fn empty_stream_urls_do_not_hide_media_sources() {
        let asset = json!({
            "stream_urls": {"Video": []},
            "media_sources": [{"label": "720", "src": "https://c/720.mp4", "type": "video/mp4"}]
        });
        assert_eq!(collect_video_sources(&asset).len(), 1);
    }

    #[test]
    fn pick_direct_closest_prefers_not_lower_on_ties() {
        let sources = vec![src("a", 480, false), src("b", 720, false), src("h", 0, true)];
        assert_eq!(pick_direct(&sources, Some(1080)).map(|s| s.height), Some(720));
        assert_eq!(pick_direct(&sources, Some(600)).map(|s| s.height), Some(720));
        assert_eq!(pick_direct(&sources, Some(360)).map(|s| s.height), Some(480));
        assert_eq!(pick_direct(&sources, None).map(|s| s.height), Some(720));
        assert_eq!(pick_direct(&[src("h", 0, true)], None), None);
    }

    #[test]
    fn prefer_hls_only_when_direct_cannot_satisfy() {
        assert!(prefer_hls(None, None));
        assert!(prefer_hls(None, Some(480)));
        assert!(prefer_hls(Some(720), None));
        assert!(prefer_hls(Some(720), Some(1080)));
        assert!(!prefer_hls(Some(720), Some(720)));
        assert!(!prefer_hls(Some(720), Some(480)));
    }

    #[test]
    fn hls_max_height_reads_master_ladder() {
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720\n720.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080\n1080.m3u8\n";
        assert_eq!(hls_max_height(master), Some(1080));
        assert_eq!(hls_max_height("#EXTM3U\n#EXTINF:10,\nseg0.ts\n"), None);
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range("bytes 100-999/1000"), Some((Some(100), Some(1000))));
        assert_eq!(parse_content_range("bytes */1000"), Some((None, Some(1000))));
        assert_eq!(parse_content_range("bytes 0-9/*"), Some((Some(0), None)));
        assert_eq!(parse_content_range("garbage"), None);
    }

    #[test]
    fn resume_plan_decisions() {
        assert_eq!(plan_resume(200, 0, None), ResumePlan::Fresh);
        assert_eq!(plan_resume(206, 500, Some("bytes 500-999/1000")), ResumePlan::Append { offset: 500 });
        assert_eq!(plan_resume(206, 500, None), ResumePlan::Append { offset: 500 });
        assert_eq!(plan_resume(206, 500, Some("bytes 0-999/1000")), ResumePlan::Fresh);
        assert_eq!(plan_resume(200, 500, None), ResumePlan::Fresh);
        assert_eq!(plan_resume(416, 1000, Some("bytes */1000")), ResumePlan::AlreadyComplete);
        assert_eq!(plan_resume(416, 1200, Some("bytes */1000")), ResumePlan::Fresh);
        assert_eq!(plan_resume(416, 500, None), ResumePlan::Fresh);
    }

    #[test]
    fn expected_total_accounts_for_offset() {
        assert_eq!(expected_total(ResumePlan::Fresh, Some(1000), None), Some(1000));
        assert_eq!(expected_total(ResumePlan::Append { offset: 500 }, Some(500), Some("bytes 500-999/1000")), Some(1000));
        assert_eq!(expected_total(ResumePlan::Append { offset: 500 }, Some(500), None), Some(1000));
        assert_eq!(expected_total(ResumePlan::Append { offset: 500 }, None, None), None);
        assert_eq!(expected_total(ResumePlan::AlreadyComplete, Some(1), None), None);
    }

    #[test]
    fn zero_tail_detection() {
        assert_eq!(zero_tail_len(&[1, 2, 0, 0, 0]), 3);
        assert_eq!(zero_tail_len(&[1, 2, 3]), 0);
        assert_eq!(zero_tail_len(&[0, 0]), 2);
        assert_eq!(zero_tail_len(&[]), 0);
    }

    #[test]
    fn trim_zero_tail_truncates_file() {
        let dir = std::env::temp_dir().join(format!("omniget-udemy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("lecture.mp4.part");
        let mut data = vec![7u8; 1000];
        data.extend(std::iter::repeat(0u8).take(300));
        std::fs::write(&part, &data).unwrap();
        assert_eq!(trim_zero_tail(&part).unwrap(), 1000);
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 1000);
        assert_eq!(trim_zero_tail(&part).unwrap(), 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_records_outcomes_with_titles() {
        let mut summary = UdemyDownloadSummary::default();
        summary.record("A", &VideoOutcome::Downloaded);
        summary.record("B", &VideoOutcome::AlreadyPresent);
        summary.record("C", &VideoOutcome::Drm);
        summary.record("D", &VideoOutcome::NoMedia("no sources".into()));
        summary.record("E", &VideoOutcome::Failed("403".into()));
        summary.record("F", &VideoOutcome::NotVideo);
        assert_eq!(summary.videos_downloaded, 1);
        assert_eq!(summary.videos_already_present, 1);
        assert_eq!(summary.drm_skipped, 1);
        assert_eq!(summary.no_media, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.drm_skipped_titles, vec!["C"]);
        assert_eq!(summary.no_media_titles, vec!["D: no sources"]);
        assert_eq!(summary.failed_titles, vec!["E: 403"]);
    }

    #[test]
    fn quality_pref_parsing() {
        assert_eq!(parse_quality_pref("best"), None);
        assert_eq!(parse_quality_pref(""), None);
        assert_eq!(parse_quality_pref("1080p"), Some(1080));
        assert_eq!(parse_quality_pref("720"), Some(720));
    }
}

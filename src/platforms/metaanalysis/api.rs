//! Meta-Analysis Academy — a custom LMS built on Supabase (GoTrue auth +
//! PostgREST) with videos hosted on Bunny Stream.
//!
//! Auth: Supabase GoTrue email/password grant. The publishable "anon" key is
//! public (shipped in the site's JS bundle) and is required on every request
//! as the `apikey` header, alongside the user's `Authorization: Bearer` token.
//! Access tokens live ~1h, so the session refreshes them transparently.
//!
//! Content: `courses` -> `modules` -> `submodules` -> `lessons`. Each lesson
//! carries HTML `content` and an optional Bunny `video_url`; attachments live
//! in `lesson_files`.
//!
//! Video: every `video_url` is a Bunny Stream embed
//! (`iframe.mediadelivery.net/embed/<lib>/<guid>` or
//! `player.mediadelivery.net/play/<lib>/<guid>`). The Bunny pull zone serves
//! `playlist.m3u8` (HLS) and `original` (source MP4) with no token/referer
//! lock, so the downloader resolves the CDN host from the embed page and pulls
//! the file directly.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0";

/// API origin (Supabase behind a custom domain).
const API_BASE: &str = "https://api.metaanalysisacademy.com";
/// Public "publishable" anon key, embedded in the site bundle.
const ANON_KEY: &str = "sb_publishable_R65Ym0eOfksnu-2g1ZKO3A_xYUsDPXg";
/// Where the SPA lives, used as Origin/Referer for good measure.
const APP_ORIGIN: &str = "https://app.metaanalysisacademy.com";

/// Refresh the access token when it is within this many seconds of expiring.
const REFRESH_SKEW_SECS: u64 = 120;

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

/// A live, authenticated session. Cheap to clone (shared token state).
#[derive(Clone)]
pub struct MetaSession {
    pub email: String,
    pub user_id: String,
    tokens: Arc<Mutex<Tokens>>,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub email: String,
    pub user_id: String,
    pub saved_at: u64,
}

// ---------------------------------------------------------------------------
// Content model (course -> module -> submodule -> lesson)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaCourse {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    /// Human-readable delivery / cohort name, shown as the card subtitle.
    pub delivery_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MetaModule {
    pub title: String,
    pub submodules: Vec<MetaSubmodule>,
}

#[derive(Debug, Clone)]
pub struct MetaSubmodule {
    pub title: String,
    pub lessons: Vec<MetaLesson>,
}

#[derive(Debug, Clone)]
pub struct MetaLesson {
    pub id: String,
    pub title: String,
    /// HTML body of the lesson, if any.
    pub content: Option<String>,
    /// Bunny Stream embed URL, if the lesson has a video.
    pub video_url: Option<String>,
    pub files: Vec<MetaFile>,
}

#[derive(Debug, Clone)]
pub struct MetaFile {
    pub name: String,
    pub url: String,
}

// ---------------------------------------------------------------------------
// Client construction
// ---------------------------------------------------------------------------

fn base_headers() -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert("apikey", HeaderValue::from_static(ANON_KEY));
    headers.insert("Accept", HeaderValue::from_static("application/json"));
    headers.insert("Origin", HeaderValue::from_static(APP_ORIGIN));
    headers.insert("Referer", HeaderValue::from_static("https://app.metaanalysisacademy.com/"));
    Ok(headers)
}

fn build_client() -> anyhow::Result<reqwest::Client> {
    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent(USER_AGENT)
        .default_headers(base_headers()?)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .build()?;
    Ok(client)
}

/// A plain client (only apikey/UA) for fetching Bunny embed pages and the CDN.
pub fn build_public_client() -> anyhow::Result<reqwest::Client> {
    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()?;
    Ok(client)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

fn parse_token_response(body: &serde_json::Value) -> anyhow::Result<(String, String, u64, String, String)> {
    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no access_token in auth response"))?
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at = body
        .get("expires_at")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| {
            let expires_in = body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
            now_secs() + expires_in
        });
    let user = body.get("user").cloned().unwrap_or(serde_json::Value::Null);
    let user_id = user
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let email = user
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((access_token, refresh_token, expires_at, user_id, email))
}

pub async fn authenticate(email: &str, password: &str) -> anyhow::Result<MetaSession> {
    let client = build_client()?;

    let resp = client
        .post(format!("{}/auth/v1/token?grant_type=password", API_BASE))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await?;

    let status = resp.status();
    let body_text = resp.text().await?;
    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| {
                v.get("error_description")
                    .or_else(|| v.get("msg"))
                    .or_else(|| v.get("error"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| body_text.chars().take(200).collect());
        return Err(anyhow!("Login failed ({}): {}", status, msg));
    }

    let body: serde_json::Value = serde_json::from_str(&body_text)?;
    let (access_token, refresh_token, expires_at, mut user_id, mut resp_email) =
        parse_token_response(&body)?;
    if resp_email.is_empty() {
        resp_email = email.to_string();
    }
    if user_id.is_empty() {
        user_id = extract_sub(&access_token).unwrap_or_default();
    }

    Ok(MetaSession {
        email: resp_email,
        user_id,
        tokens: Arc::new(Mutex::new(Tokens {
            access_token,
            refresh_token,
            expires_at,
        })),
        client,
    })
}

/// Best-effort extraction of the `sub` (user id) claim from a JWT.
fn extract_sub(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload_b64)?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    json.get("sub").and_then(|v| v.as_str()).map(String::from)
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    // Minimal URL-safe base64 decoder (no padding required), avoiding an extra
    // dependency just for the JWT payload.
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut lut = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lut[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        let v = lut[c as usize];
        if v == 255 {
            continue;
        }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

impl MetaSession {
    /// Refresh the access token when it is close to expiry. Called before each
    /// authenticated request so long downloads never fail on a stale token.
    async fn ensure_fresh(&self) -> anyhow::Result<()> {
        let (needs_refresh, refresh_token) = {
            let guard = self.tokens.lock().await;
            (
                guard.expires_at <= now_secs() + REFRESH_SKEW_SECS,
                guard.refresh_token.clone(),
            )
        };
        if !needs_refresh || refresh_token.is_empty() {
            return Ok(());
        }

        let resp = self
            .client
            .post(format!("{}/auth/v1/token?grant_type=refresh_token", API_BASE))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await?;

        if !resp.status().is_success() {
            // Keep the old token; the caller surfaces a 401 if it is truly dead.
            tracing::warn!("[metaanalysis] token refresh failed: {}", resp.status());
            return Ok(());
        }

        let body: serde_json::Value = resp.json().await?;
        let (access_token, new_refresh, expires_at, _uid, _email) = parse_token_response(&body)?;
        let mut guard = self.tokens.lock().await;
        guard.access_token = access_token;
        if !new_refresh.is_empty() {
            guard.refresh_token = new_refresh;
        }
        guard.expires_at = expires_at;
        tracing::info!("[metaanalysis] access token refreshed");
        Ok(())
    }

    async fn access_token(&self) -> String {
        self.tokens.lock().await.access_token.clone()
    }

    fn saved(&self, tokens: &Tokens) -> SavedSession {
        SavedSession {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: tokens.expires_at,
            email: self.email.clone(),
            user_id: self.user_id.clone(),
            saved_at: now_secs(),
        }
    }

    /// Authenticated GET returning parsed JSON. Refreshes the token first.
    async fn get_json(&self, url: &str) -> anyhow::Result<serde_json::Value> {
        self.ensure_fresh().await?;
        let token = self.access_token().await;
        let resp = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        let status = resp.status();
        let body_text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "GET {} -> {}: {}",
                url,
                status,
                body_text.chars().take(200).collect::<String>()
            ));
        }
        Ok(serde_json::from_str(&body_text)?)
    }

    /// Authenticated POST (used for RPC calls). Refreshes the token first.
    async fn post_json(&self, url: &str, payload: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.ensure_fresh().await?;
        let token = self.access_token().await;
        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(payload)
            .send()
            .await?;
        let status = resp.status();
        let body_text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "POST {} -> {}: {}",
                url,
                status,
                body_text.chars().take(200).collect::<String>()
            ));
        }
        Ok(serde_json::from_str(&body_text)?)
    }

    /// A reqwest client suitable for downloading public files (attachments and
    /// the Bunny CDN). No auth header, but honors the global proxy.
    pub fn public_client(&self) -> anyhow::Result<reqwest::Client> {
        build_public_client()
    }
}

// ---------------------------------------------------------------------------
// Session validation & persistence
// ---------------------------------------------------------------------------

/// Validate the session by hitting the GoTrue userinfo endpoint. Returns the
/// account email on success.
pub async fn validate(session: &MetaSession) -> anyhow::Result<String> {
    let user = session
        .get_json(&format!("{}/auth/v1/user", API_BASE))
        .await?;
    let email = user
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or(&session.email)
        .to_string();
    Ok(email)
}

fn session_file_path() -> anyhow::Result<PathBuf> {
    let data_dir = dirs::data_dir().ok_or_else(|| anyhow!("Could not find app data directory"))?;
    Ok(data_dir
        .join("wtf.tonho.omniget")
        .join("metaanalysis_session.json"))
}

pub async fn save_session(session: &MetaSession) -> anyhow::Result<()> {
    let path = session_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let saved = {
        let guard = session.tokens.lock().await;
        session.saved(&guard)
    };
    std::fs::write(&path, serde_json::to_string_pretty(&saved)?)?;
    tracing::info!("[metaanalysis] session saved for {}", session.email);
    Ok(())
}

pub async fn load_session() -> anyhow::Result<Option<MetaSession>> {
    let path = session_file_path()?;
    let json = match std::fs::read_to_string(&path) {
        Ok(j) => j,
        Err(_) => return Ok(None),
    };
    let saved: SavedSession = serde_json::from_str(&json)?;
    let client = build_client()?;
    Ok(Some(MetaSession {
        email: saved.email,
        user_id: saved.user_id,
        tokens: Arc::new(Mutex::new(Tokens {
            access_token: saved.access_token,
            refresh_token: saved.refresh_token,
            expires_at: saved.expires_at,
        })),
        client,
    }))
}

pub async fn delete_saved_session() -> anyhow::Result<()> {
    let path = session_file_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Course listing
// ---------------------------------------------------------------------------

pub async fn list_courses(session: &MetaSession) -> anyhow::Result<Vec<MetaCourse>> {
    let body = session
        .post_json(
            &format!("{}/rest/v1/rpc/get_user_accessible_courses", API_BASE),
            &serde_json::json!({ "_user_id": session.user_id }),
        )
        .await?;

    let arr = body
        .as_array()
        .ok_or_else(|| anyhow!("unexpected courses response"))?;

    let mut courses = Vec::new();
    for c in arr {
        let id = c
            .get("course_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let name = c
            .get("course_title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled course")
            .to_string();
        let image_url = c
            .get("course_thumbnail_url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let delivery_name = c
            .get("delivery_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        courses.push(MetaCourse {
            id,
            name,
            image_url,
            delivery_name,
        });
    }
    Ok(courses)
}

// ---------------------------------------------------------------------------
// Course content (modules -> submodules -> lessons -> files)
// ---------------------------------------------------------------------------

fn rest_url(path_and_query: &str) -> String {
    format!("{}/rest/v1/{}", API_BASE, path_and_query)
}

/// PostgREST `in.(a,b,c)` filter value for a list of UUIDs.
fn in_list(ids: &[String]) -> String {
    format!("({})", ids.join(","))
}

pub async fn get_course_content(
    session: &MetaSession,
    course_id: &str,
) -> anyhow::Result<Vec<MetaModule>> {
    // 1) Modules of the course (active or locked, ordered).
    let modules_json = session
        .get_json(&rest_url(&format!(
            "modules?select=id,title,order_index&course_id=eq.{}&status=in.(active,locked)&order=order_index.asc",
            course_id
        )))
        .await?;
    let module_rows = modules_json.as_array().cloned().unwrap_or_default();
    if module_rows.is_empty() {
        return Ok(Vec::new());
    }

    let module_ids: Vec<String> = module_rows
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();

    // 2) Submodules for those modules.
    let submodule_rows = fetch_in_batches(
        session,
        &module_ids,
        "submodules?select=id,module_id,title,order_index&is_active=eq.true&module_id=in.",
        "&order=order_index.asc",
    )
    .await?;

    let submodule_ids: Vec<String> = submodule_rows
        .iter()
        .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();

    // 3) Lessons for those submodules.
    let lesson_rows = fetch_in_batches(
        session,
        &submodule_ids,
        "lessons?select=id,submodule_id,title,content,video_url,order_index&is_active=eq.true&submodule_id=in.",
        "&order=order_index.asc",
    )
    .await?;

    let lesson_ids: Vec<String> = lesson_rows
        .iter()
        .filter_map(|l| l.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();

    // 4) Attachments for those lessons.
    let file_rows = fetch_in_batches(
        session,
        &lesson_ids,
        "lesson_files?select=id,lesson_id,name,file_url&lesson_id=in.",
        "",
    )
    .await?;

    // Index files by lesson id.
    let mut files_by_lesson: std::collections::HashMap<String, Vec<MetaFile>> =
        std::collections::HashMap::new();
    for f in &file_rows {
        let lesson_id = f.get("lesson_id").and_then(|v| v.as_str()).unwrap_or("");
        let url = f.get("file_url").and_then(|v| v.as_str()).unwrap_or("");
        if lesson_id.is_empty() || url.is_empty() {
            continue;
        }
        let name = f
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| filename_from_url(url));
        files_by_lesson
            .entry(lesson_id.to_string())
            .or_default()
            .push(MetaFile {
                name,
                url: url.to_string(),
            });
    }

    // Index lessons by submodule id (already ordered by the query).
    let mut lessons_by_submodule: std::collections::HashMap<String, Vec<MetaLesson>> =
        std::collections::HashMap::new();
    for l in &lesson_rows {
        let submodule_id = l.get("submodule_id").and_then(|v| v.as_str()).unwrap_or("");
        let id = l.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if submodule_id.is_empty() || id.is_empty() {
            continue;
        }
        let title = l
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .trim()
            .to_string();
        let content = l
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(String::from);
        let video_url = l
            .get("video_url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(String::from);
        let files = files_by_lesson.remove(id).unwrap_or_default();
        lessons_by_submodule
            .entry(submodule_id.to_string())
            .or_default()
            .push(MetaLesson {
                id: id.to_string(),
                title,
                content,
                video_url,
                files,
            });
    }

    // Index submodules by module id (already ordered).
    let mut submodules_by_module: std::collections::HashMap<String, Vec<MetaSubmodule>> =
        std::collections::HashMap::new();
    for s in &submodule_rows {
        let module_id = s.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if module_id.is_empty() || id.is_empty() {
            continue;
        }
        let title = s
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .trim()
            .to_string();
        let lessons = lessons_by_submodule.remove(id).unwrap_or_default();
        submodules_by_module
            .entry(module_id.to_string())
            .or_default()
            .push(MetaSubmodule { title, lessons });
    }

    // Assemble in module order.
    let mut modules = Vec::new();
    for m in &module_rows {
        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let title = m
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .trim()
            .to_string();
        let submodules = submodules_by_module.remove(id).unwrap_or_default();
        modules.push(MetaModule { title, submodules });
    }

    Ok(modules)
}

/// Run a PostgREST `in.(...)` query in chunks so the URL never grows unbounded.
async fn fetch_in_batches(
    session: &MetaSession,
    ids: &[String],
    prefix: &str,
    suffix: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    if ids.is_empty() {
        return Ok(out);
    }
    for chunk in ids.chunks(40) {
        let url = rest_url(&format!("{}{}{}", prefix, in_list(chunk), suffix));
        let json = session.get_json(&url).await?;
        if let Some(arr) = json.as_array() {
            out.extend(arr.iter().cloned());
        }
    }
    Ok(out)
}

fn filename_from_url(url: &str) -> String {
    url.split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .to_string()
}

// ---------------------------------------------------------------------------
// Bunny Stream resolution
// ---------------------------------------------------------------------------

/// The direct download targets resolved from a Bunny embed URL.
pub struct BunnyMedia {
    /// Source MP4 (`.../<guid>/original`) when the library exposes it.
    pub original_url: Option<String>,
    /// HLS master playlist (`.../<guid>/playlist.m3u8`).
    pub hls_url: Option<String>,
}

/// Parse the library id and video GUID from either Bunny embed URL shape.
fn parse_bunny_embed(video_url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(video_url).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    if !host.ends_with("mediadelivery.net") {
        return None;
    }
    // Path is /embed/<lib>/<guid> or /play/<lib>/<guid>.
    let segs: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() >= 3 && (segs[0] == "embed" || segs[0] == "play") {
        return Some((segs[1].to_string(), segs[2].to_string()));
    }
    None
}

/// Resolve a Bunny embed URL to direct CDN download targets by reading the
/// embed page (which exposes the pull-zone hostname).
pub async fn resolve_bunny(
    client: &reqwest::Client,
    video_url: &str,
) -> anyhow::Result<BunnyMedia> {
    let (library_id, guid) =
        parse_bunny_embed(video_url).ok_or_else(|| anyhow!("not a Bunny Stream URL: {}", video_url))?;

    let embed_url = format!(
        "https://iframe.mediadelivery.net/embed/{}/{}",
        library_id, guid
    );

    let html = client
        .get(&embed_url)
        .header("Referer", "https://app.metaanalysisacademy.com/")
        .send()
        .await?
        .text()
        .await?;

    // Find the pull-zone hostname (e.g. vz-7b69c07d-2f4.b-cdn.net) referenced
    // in the player config for this guid.
    let pull_zone = find_pull_zone(&html, &guid);

    let (original_url, hls_url) = match pull_zone {
        Some(host) => (
            Some(format!("https://{}/{}/original", host, guid)),
            Some(format!("https://{}/{}/playlist.m3u8", host, guid)),
        ),
        None => (None, None),
    };

    if original_url.is_none() && hls_url.is_none() {
        return Err(anyhow!(
            "could not resolve Bunny CDN host for {}",
            video_url
        ));
    }

    Ok(BunnyMedia {
        original_url,
        hls_url,
    })
}

/// Extract `vz-XXXX.b-cdn.net` from the embed HTML, preferring one that appears
/// next to this video's guid.
fn find_pull_zone(html: &str, guid: &str) -> Option<String> {
    // Look for the canonical playlist URL first.
    let needle = "b-cdn.net";
    let mut search_from = 0;
    let mut fallback: Option<String> = None;
    while let Some(rel) = html[search_from..].find(needle) {
        let idx = search_from + rel;
        // Walk backwards to the start of the hostname.
        let start = html[..idx]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
            .map(|p| p + 1)
            .unwrap_or(0);
        let end = idx + needle.len();
        let host = &html[start..end];
        if host.starts_with("vz-") && host.ends_with("b-cdn.net") {
            let host = host.to_string();
            // Prefer a hostname whose surrounding text mentions this guid.
            let window_start = idx.saturating_sub(200);
            let window_end = (end + 200).min(html.len());
            if html[window_start..window_end].contains(guid) {
                return Some(host);
            }
            fallback.get_or_insert(host);
        }
        search_from = end;
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iframe_embed_url() {
        let (lib, guid) =
            parse_bunny_embed("https://iframe.mediadelivery.net/embed/652325/abc-123").unwrap();
        assert_eq!(lib, "652325");
        assert_eq!(guid, "abc-123");
    }

    #[test]
    fn parses_player_play_url() {
        let (lib, guid) =
            parse_bunny_embed("https://player.mediadelivery.net/play/652325/def-456?x=1").unwrap();
        assert_eq!(lib, "652325");
        assert_eq!(guid, "def-456");
    }

    #[test]
    fn rejects_non_bunny_url() {
        assert!(parse_bunny_embed("https://youtube.com/watch?v=x").is_none());
    }

    #[test]
    fn finds_pull_zone_near_guid() {
        let guid = "540da72c-dd91-46f5-9cfc-b89db288f345";
        let html = format!(
            "<video><source src=\"https://vz-7b69c07d-2f4.b-cdn.net/{}/playlist.m3u8\"></video>",
            guid
        );
        assert_eq!(
            find_pull_zone(&html, guid).as_deref(),
            Some("vz-7b69c07d-2f4.b-cdn.net")
        );
    }

    #[test]
    fn falls_back_to_any_pull_zone() {
        let html = "cfg = {host:\"vz-abcdef12-3f4.b-cdn.net\"}";
        assert_eq!(
            find_pull_zone(html, "no-such-guid").as_deref(),
            Some("vz-abcdef12-3f4.b-cdn.net")
        );
    }

    #[test]
    fn decodes_jwt_sub() {
        // {"sub":"user-42"} base64url, no padding.
        let payload = "eyJzdWIiOiJ1c2VyLTQyIn0";
        let jwt = format!("header.{}.sig", payload);
        assert_eq!(extract_sub(&jwt).as_deref(), Some("user-42"));
    }
}

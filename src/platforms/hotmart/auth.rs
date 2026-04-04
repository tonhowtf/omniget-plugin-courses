#[cfg(windows)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use reqwest::cookie::Jar;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};


#[derive(Clone)]
pub struct HotmartSession {
    pub token: String,
    pub email: String,
    pub client: reqwest::Client,
    pub cookies: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub token: String,
    pub email: String,
    pub cookies: Vec<(String, String)>,
    pub saved_at: u64,
}

fn session_file_path() -> anyhow::Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow!("Could not find app data directory"))?;
    Ok(data_dir.join("wtf.tonho.omniget").join("hotmart_session.json"))
}

pub fn build_client_from_saved(saved: &SavedSession) -> anyhow::Result<reqwest::Client> {
    let jar = Jar::default();
    let domains = [
        "https://hotmart.com",
        "https://api-sec-vlc.hotmart.com",
        "https://api-hub.cb.hotmart.com",
        "https://api-club-course-consumption-gateway-ga.cb.hotmart.com",
        "https://consumer.hotmart.com",
        "https://api-club-hot-club-api.cb.hotmart.com",
    ];
    for (name, value) in &saved.cookies {
        let cookie_str = format!("{}={}; Domain=.hotmart.com; Path=/", name, value);
        for domain in &domains {
            jar.add_cookie_str(&cookie_str, &domain.parse().unwrap());
        }
    }

    let mut default_headers = HeaderMap::new();
    default_headers.insert(
        "Accept",
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    default_headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", saved.token))?,
    );
    default_headers.insert(
        "Origin",
        HeaderValue::from_static("https://consumer.hotmart.com"),
    );
    default_headers.insert(
        "Referer",
        HeaderValue::from_static("https://consumer.hotmart.com"),
    );
    default_headers.insert("Pragma", HeaderValue::from_static("no-cache"));
    default_headers.insert("cache-control", HeaderValue::from_static("no-cache"));

    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .cookie_provider(Arc::new(jar))
        .default_headers(default_headers)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()?;

    Ok(client)
}

pub async fn save_session(session: &HotmartSession) -> anyhow::Result<()> {
    let path = session_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let saved = SavedSession {
        token: session.token.clone(),
        email: session.email.clone(),
        cookies: session.cookies.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    let json = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, json)?;
    tracing::info!("[session] saved for {}, {} cookies", session.email, session.cookies.len());
    Ok(())
}

pub async fn load_saved_session() -> anyhow::Result<HotmartSession> {
    let path = session_file_path()?;
    let json = std::fs::read_to_string(&path)?;
    let saved: SavedSession = serde_json::from_str(&json)?;

    tracing::info!("[session] loaded for {}, {} cookies", saved.email, saved.cookies.len());

    let client = build_client_from_saved(&saved)?;

    Ok(HotmartSession {
        token: saved.token,
        email: saved.email,
        cookies: saved.cookies,
        client,
    })
}

pub async fn delete_saved_session() -> anyhow::Result<()> {
    let path = session_file_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(windows)]
const COOKIE_URIS: &[&str] = &[
    "https://hotmart.com",
    "https://sso.hotmart.com",
    "https://consumer.hotmart.com",
    "https://api-sec-vlc.hotmart.com",
];

#[cfg(windows)]
async fn extract_webview_cookies_for_uri(
        _uri: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    Err(anyhow!("Webview cookie extraction not available in plugin mode"))
}

#[cfg(windows)]
async fn extract_webview_cookies(
    ) -> anyhow::Result<Vec<(String, String)>> {
    Err(anyhow!("Webview cookie extraction not available in plugin mode"))
}

#[allow(dead_code)]
fn parse_document_cookie(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let (name, value) = pair.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

#[cfg(not(windows))]
async fn extract_webview_cookies_js(
        _cookie_data: &Arc<std::sync::Mutex<Option<String>>>,
) -> anyhow::Result<Vec<(String, String)>> {
    Err(anyhow!("Webview cookie extraction not available in plugin mode"))
}

pub async fn authenticate(
    host: &std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    email: &str,
    password: &str,
) -> anyhow::Result<HotmartSession> {
    tracing::info!("[hotmart] authenticate: attempting login for {}", email);

    let login_client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| anyhow!("Failed to build login client: {}", e))?;

    let resp = login_client
        .post("https://api-sec-vlc.hotmart.com/security/oauth/login")
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", "https://app.hotmart.com")
        .header("Referer", "https://app.hotmart.com/")
        .form(&[
            ("grant_type", "password"),
            ("username", email),
            ("password", password),
        ])
        .send()
        .await
        .map_err(|e| anyhow!("Login request failed: {}", e))?;

    let status = resp.status();
    let body_text = resp.text().await
        .map_err(|e| anyhow!("Failed to read login response: {}", e))?;

    tracing::info!("[hotmart] login response status: {}, body: {}", status, &body_text[..body_text.len().min(500)]);

    if !status.is_success() {
        let body: serde_json::Value = serde_json::from_str(&body_text).unwrap_or_default();

        if body_text.to_lowercase().contains("captcha")
            || status.as_u16() == 403
        {
            tracing::warn!("[hotmart] captcha required during login");
            let _ = host.emit_event(
                "hotmart-auth-captcha",
                serde_json::json!({"message": "Captcha required. Please use browser login."}),
            );
            return Err(anyhow!("Captcha required. Please use browser login instead."));
        }

        let error_msg = body
            .get("message")
            .or_else(|| body.get("error_description"))
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.get("message")).and_then(|v| v.as_str()))
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or(&body_text[..body_text.len().min(200)]);

        return Err(anyhow!("Authentication failed (status {}): {}", status, error_msg));
    }

    let body: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|e| anyhow!("Failed to parse login response: {}", e))?;

    let token = body
        .get("access_token")
        .or_else(|| body.get("token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("No access_token in login response. Keys: {:?}", body.as_object().map(|o| o.keys().collect::<Vec<_>>())))?
        .to_string();

    let cookies = vec![("access_token".to_string(), token.clone())];

    let saved = SavedSession {
        token: token.clone(),
        email: email.to_string(),
        cookies: cookies.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    let client = build_client_from_saved(&saved)
        .map_err(|e| anyhow!("Failed to build session client: {}", e))?;

    tracing::info!("[hotmart] login successful for {}", email);

    Ok(HotmartSession {
        token,
        email: email.to_string(),
        client,
        cookies,
    })
}

#[cfg(windows)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};


#[derive(Clone)]
pub struct UdemySession {
    pub access_token: String,
    pub email: String,
    pub client: reqwest::Client,
    pub cookies: Vec<(String, String)>,
    pub portal_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub access_token: String,
    pub email: String,
    pub cookies: Vec<(String, String)>,
    pub saved_at: u64,
    #[serde(default = "default_portal")]
    pub portal_name: String,
}

fn default_portal() -> String {
    "www".into()
}

fn session_file_path() -> anyhow::Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow!("Could not find app data directory"))?;
    Ok(data_dir.join("omniget").join("udemy_session.json"))
}

const UDEMY_CLIENT_ID: &str = "TH96Ov3Ebo3OtgoSH5mOYzYolcowM3ycedWQDDce";
const UDEMY_CLIENT_SECRET: &str = "f2lgDUDxjFiOlVHUpwQNFUfCQPyMO0tJQMaud53PF01UKueW8enYjeEYoyVeP0bb2XVEDkJ5GLJaVTfM5QgMVz6yyXyydZdA5QhzgvG9UmCPUYaCrIVf7VpmiilfbLJc";

pub fn build_client_from_saved(saved: &SavedSession) -> anyhow::Result<reqwest::Client> {
    if saved.portal_name != "www" {
        return build_enterprise_client(saved);
    }

    let mut default_headers = HeaderMap::new();

    if !saved.access_token.is_empty() {
        let bearer = format!("Bearer {}", saved.access_token);
        default_headers.insert(
            "Authorization",
            HeaderValue::from_str(&bearer)?,
        );
        default_headers.insert(
            "X-Udemy-Authorization",
            HeaderValue::from_str(&bearer)?,
        );
    }

    default_headers.insert(
        "Accept",
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    default_headers.insert(
        "x-udemy-client-id",
        HeaderValue::from_static(UDEMY_CLIENT_ID),
    );
    default_headers.insert(
        "x-udemy-client-secret",
        HeaderValue::from_static(UDEMY_CLIENT_SECRET),
    );
    default_headers.insert(
        "accept-language",
        HeaderValue::from_static("en_US"),
    );

    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent("okhttp/4.12.0 UdemyAndroid 9.51.2(594) (phone)")
        .default_headers(default_headers)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()?;

    Ok(client)
}

fn build_enterprise_client(saved: &SavedSession) -> anyhow::Result<reqwest::Client> {
    let mut default_headers = HeaderMap::new();

    let cookie_header = saved
        .cookies
        .iter()
        .map(|(name, value)| format!("{}={}", name, value))
        .collect::<Vec<_>>()
        .join("; ");

    if !cookie_header.is_empty() {
        default_headers.insert("Cookie", HeaderValue::from_str(&cookie_header)?);
    }

    if let Some(csrf) = saved.cookies.iter().find(|(n, _)| n == "csrftoken") {
        default_headers.insert("X-CSRFToken", HeaderValue::from_str(&csrf.1)?);
    }

    let origin = format!("https://{}.udemy.com", saved.portal_name);
    default_headers.insert("Referer", HeaderValue::from_str(&format!("{}/", origin))?);
    default_headers.insert("Origin", HeaderValue::from_str(&origin)?);
    default_headers.insert(
        "x-requested-with",
        HeaderValue::from_static("XMLHttpRequest"),
    );
    default_headers.insert(
        "Accept",
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    default_headers.insert(
        "accept-language",
        HeaderValue::from_static("en_US"),
    );

    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .default_headers(default_headers)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()?;

    Ok(client)
}

pub async fn save_session(session: &UdemySession) -> anyhow::Result<()> {
    let path = session_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let saved = SavedSession {
        access_token: session.access_token.clone(),
        email: session.email.clone(),
        cookies: session.cookies.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        portal_name: session.portal_name.clone(),
    };

    let json = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, json)?;
    tracing::info!("[udemy] session saved for {}, {} cookies", session.email, session.cookies.len());
    Ok(())
}

pub async fn load_saved_session() -> anyhow::Result<UdemySession> {
    let path = session_file_path()?;
    let json = std::fs::read_to_string(&path)?;
    let saved: SavedSession = serde_json::from_str(&json)?;

    tracing::info!("[udemy] session loaded for {}, {} cookies", saved.email, saved.cookies.len());

    let client = build_client_from_saved(&saved)?;

    Ok(UdemySession {
        access_token: saved.access_token,
        email: saved.email,
        cookies: saved.cookies,
        client,
        portal_name: saved.portal_name,
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
    "https://www.udemy.com",
    "https://udemy.com",
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

fn strip_cookie_quotes(value: &str) -> &str {
    let v = value.trim();
    if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

pub async fn request_otp(email: &str) -> anyhow::Result<()> {
    tracing::info!("[udemy] requesting OTP for {}", email);

    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent("okhttp/4.12.0 UdemyAndroid 9.51.2(594) (phone)")
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| anyhow!("Failed to build OTP client: {}", e))?;

    let resp = client
        .post("https://www.udemy.com/api-2.0/auth/udemy-provisional/")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-udemy-client-id", UDEMY_CLIENT_ID)
        .header("x-udemy-client-secret", UDEMY_CLIENT_SECRET)
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .map_err(|e| anyhow!("OTP request failed: {}", e))?;

    let status = resp.status();
    tracing::info!("[udemy] OTP request response: {}", status);

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("OTP request failed ({}): {}", status, &body[..body.len().min(300)]));
    }

    tracing::info!("[udemy] OTP sent to {}", email);
    Ok(())
}

pub async fn verify_otp(email: &str, otp_code: &str) -> anyhow::Result<UdemySession> {
    tracing::info!("[udemy] verifying OTP for {}", email);

    let client = omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent("okhttp/4.12.0 UdemyAndroid 9.51.2(594) (phone)")
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| anyhow!("Failed to build verify client: {}", e))?;

    let resp = client
        .post("https://www.udemy.com/api-2.0/auth/code-verification/")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-udemy-client-id", UDEMY_CLIENT_ID)
        .header("x-udemy-client-secret", UDEMY_CLIENT_SECRET)
        .json(&serde_json::json!({
            "email": email,
            "otp": otp_code
        }))
        .send()
        .await
        .map_err(|e| anyhow!("OTP verification request failed: {}", e))?;

    let status = resp.status();
    let body_text = resp.text().await
        .map_err(|e| anyhow!("Failed to read verify response: {}", e))?;

    tracing::info!("[udemy] OTP verify response: {}", status);

    if !status.is_success() {
        return Err(anyhow!("OTP verification failed ({}): {}", status, &body_text[..body_text.len().min(300)]));
    }

    let body: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|e| anyhow!("Failed to parse verify response: {}", e))?;

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("No access_token in OTP verify response"))?
        .to_string();

    let cookies = vec![("access_token".to_string(), access_token.clone())];

    let saved = SavedSession {
        access_token: access_token.clone(),
        email: email.to_string(),
        cookies: cookies.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        portal_name: "www".to_string(),
    };

    let session_client = build_client_from_saved(&saved)?;

    tracing::info!("[udemy] OTP login successful for {}", email);

    Ok(UdemySession {
        access_token,
        email: email.to_string(),
        client: session_client,
        cookies,
        portal_name: "www".to_string(),
    })
}

pub async fn authenticate(
    _host: &std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    _email: &str,
) -> anyhow::Result<UdemySession> {
    Err(anyhow!("Use udemy_request_otp + udemy_verify_otp instead. The email-only login requires a two-step OTP flow."))
}

pub async fn authenticate_with_cookie_json(cookie_json_str: &str) -> anyhow::Result<UdemySession> {
    #[derive(Deserialize)]
    struct CookieEntry {
        name: String,
        value: String,
        #[allow(dead_code)]
        domain: Option<String>,
    }

    #[derive(Deserialize)]
    struct CookieExport {
        url: Option<String>,
        cookies: Vec<CookieEntry>,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CookieInput {
        Export(CookieExport),
        CookieArray(Vec<CookieEntry>),
    }

    tracing::info!("[udemy] authenticate_with_cookie_json: json size={}", cookie_json_str.len());

    let input: CookieInput = serde_json::from_str(cookie_json_str).map_err(|e| anyhow!("Invalid cookie JSON: {}", e))?;

    let export = match input {
        CookieInput::Export(export) => export,
        CookieInput::CookieArray(cookies) => CookieExport { url: None, cookies },
    };

    tracing::info!("[udemy] parsed {} cookies", export.cookies.len());

    if export.cookies.is_empty() {
        return Err(anyhow!("No cookies provided"));
    }

    let cookie_names: Vec<&str> = export.cookies.iter().map(|c| c.name.as_str()).collect();
    tracing::info!("[udemy] cookie names: {:?}", cookie_names);

    let portal_name = if let Some(ref url) = export.url {
        detect_portal_from_url(url)
    } else {
        export
            .cookies
            .iter()
            .filter_map(|c| c.domain.as_deref())
            .find_map(|d| {
                let d = d.trim_start_matches('.');
                d.strip_suffix(".udemy.com")
                    .map(|sub| sub.to_string())
            })
            .unwrap_or_else(|| "www".into())
    };

    let cookies: Vec<(String, String)> = export
        .cookies
        .iter()
        .map(|c| (c.name.clone(), c.value.clone()))
        .collect();

    let access_token = cookies
        .iter()
        .find(|(name, _)| name == "access_token")
        .map(|(_, value)| strip_cookie_quotes(value).to_string())
        .unwrap_or_default();

    if access_token.is_empty() {
        tracing::warn!("[udemy] no access_token cookie found, enterprise login will rely on cookies only");
    }

    let email = cookies
        .iter()
        .find(|(name, _)| name == "ud_user_jwt")
        .and_then(|(_, value)| decode_jwt_email(value))
        .unwrap_or_else(|| {
            tracing::info!("[udemy] JWT email decode failed or ud_user_jwt not found, using fallback email");
            format!("enterprise@{}.udemy.com", portal_name)
        });

    let saved = SavedSession {
        access_token: access_token.clone(),
        email: email.clone(),
        cookies: cookies.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        portal_name: portal_name.clone(),
    };

    let client = build_client_from_saved(&saved)?;

    tracing::info!(
        "[udemy] cookie login: portal={}, email={}, {} cookies, has_access_token={}",
        portal_name,
        email,
        cookies.len(),
        !access_token.is_empty()
    );

    Ok(UdemySession {
        access_token,
        email,
        client,
        cookies,
        portal_name,
    })
}

fn detect_portal_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .and_then(|host| {
            host.strip_suffix(".udemy.com")
                .map(|sub| sub.to_string())
        })
        .unwrap_or_else(|| "www".into())
}

fn decode_jwt_email(jwt: &str) -> Option<String> {
    use base64::Engine;

    // JWT has 3 parts: header.payload.signature
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(parts[1]))
        .ok()?;

    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

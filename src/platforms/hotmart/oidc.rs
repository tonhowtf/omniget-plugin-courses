//! Hotmart moved its consumer area (consumer.hotmart.com) and the Club player
//! to OpenID Connect against `sso.hotmart.com/oidc` (CAS). The browser keeps
//! the session in `localStorage["oidc.user:<authority>:<client_id>"]`, not in
//! the `hmVlcIntegration` cookie the plugin used to look for, and the legacy
//! `check_token` endpoint rejects these tokens. This module knows how to read
//! that entry, validate a token against the OIDC userinfo endpoint and, given
//! the SSO cookies, mint a fresh token through the public PKCE client the
//! consumer app itself uses (no client secret is involved).

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use base64::Engine;
use reqwest::cookie::Jar;
use sha2::{Digest, Sha256};

pub const AUTHORITY: &str = "https://sso.hotmart.com/oidc";
pub const CONSUMER_CLIENT_ID: &str = "0fff6c2a-971c-4f7a-b0b3-3032b7a26319";
pub const REDIRECT_URI: &str = "https://consumer.hotmart.com/auth/login";
pub const SCOPE: &str = "openid profile authorities email user";
pub const STORAGE_KEY_PREFIX: &str = "oidc.user:";

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const MAX_SSO_HOPS: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct OidcUser {
    pub access_token: String,
    pub expires_at: Option<u64>,
    pub email: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub email: Option<String>,
    pub name: Option<String>,
    pub id: Option<String>,
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Parses the JSON that oidc-client stores under `oidc.user:…`.
pub fn parse_user_entry(value: &str) -> Option<OidcUser> {
    let v: serde_json::Value = serde_json::from_str(value).ok()?;
    let access_token = v.get("access_token")?.as_str()?.trim().to_string();
    if access_token.is_empty() {
        return None;
    }
    let profile = v.get("profile");
    let pick = |k: &str| -> Option<String> {
        profile
            .and_then(|p| p.get(k))
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    Some(OidcUser {
        access_token,
        expires_at: v.get("expires_at").and_then(|e| e.as_u64()),
        email: pick("email").or_else(|| pick("login")),
        name: pick("name"),
    })
}

/// Finds the `oidc.user:…` entry among the name/value pairs the auth webview
/// (or a pasted export) handed us.
pub fn find_user_entry(pairs: &[(String, String)]) -> Option<OidcUser> {
    pairs
        .iter()
        .filter(|(name, _)| {
            let n = name.strip_prefix("ss:").unwrap_or(name);
            n.starts_with(STORAGE_KEY_PREFIX)
        })
        .find_map(|(_, value)| parse_user_entry(value))
}

fn base_client() -> reqwest::ClientBuilder {
    omniget_core::core::http_client::apply_global_proxy(reqwest::Client::builder())
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
}

/// `GET /oidc/profile` — the userinfo endpoint. 200 means the token is alive.
pub async fn fetch_profile(client: &reqwest::Client, token: &str) -> anyhow::Result<Profile> {
    let resp = client
        .get(format!("{AUTHORITY}/profile"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| anyhow!("userinfo request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("userinfo returned status {status}"));
    }
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok(profile_from_userinfo(&body))
}

pub fn profile_from_userinfo(body: &serde_json::Value) -> Profile {
    let attrs = body.get("attributes");
    let pick = |k: &str| -> Option<String> {
        body.get(k)
            .or_else(|| attrs.and_then(|a| a.get(k)))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    Profile {
        email: pick("email").or_else(|| pick("login")),
        name: pick("name"),
        id: pick("id").or_else(|| pick("sub")),
    }
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

pub fn new_pkce() -> Pkce {
    let verifier = b64url(&rand::random::<[u8; 32]>());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = b64url(&rand::random::<[u8; 16]>());
    Pkce {
        verifier,
        challenge,
        state,
    }
}

pub fn authorize_url(pkce: &Pkce) -> String {
    let q = [
        ("client_id", CONSUMER_CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("state", pkce.state.as_str()),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("response_mode", "query"),
    ];
    let query = q
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{AUTHORITY}/authorize?{query}")
}

/// Extracts `code` from the redirect back to the consumer app, checking it is
/// really that redirect and that the state round-tripped.
pub fn code_from_redirect(location: &str, expected_state: &str) -> Option<String> {
    let url = url::Url::parse(location).ok()?;
    if url.host_str() != Some("consumer.hotmart.com") {
        return None;
    }
    let mut code = None;
    let mut state_ok = false;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" if !v.is_empty() => code = Some(v.to_string()),
            "state" => state_ok = v == expected_state,
            _ => {}
        }
    }
    if state_ok {
        code
    } else {
        None
    }
}

/// Runs the authorization-code + PKCE flow with an existing SSO session
/// (the cookies captured from the login window or exported from a browser).
/// Returns a fresh access token without any UI.
pub async fn exchange_with_cookies(cookies: &[(String, String)]) -> anyhow::Result<OidcUser> {
    if cookies.is_empty() {
        return Err(anyhow!("no SSO cookies to renew the session with"));
    }
    let jar = Jar::default();
    let sso: url::Url = "https://sso.hotmart.com".parse().expect("static url");
    for (name, value) in cookies {
        if name.starts_with(STORAGE_KEY_PREFIX) || name.starts_with("ss:") || name.contains(':') {
            continue;
        }
        jar.add_cookie_str(
            &format!("{name}={value}; Domain=.hotmart.com; Path=/"),
            &sso,
        );
    }
    let client = base_client()
        .cookie_provider(Arc::new(jar))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let pkce = new_pkce();
    let mut next = authorize_url(&pkce);
    let mut code = None;
    for _ in 0..MAX_SSO_HOPS {
        let resp = client
            .get(&next)
            .header("Accept", "text/html,application/xhtml+xml,*/*")
            .send()
            .await?;
        let status = resp.status();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let Some(location) = location else {
            if status.is_success() {
                return Err(anyhow!(
                    "SSO answered the authorize request with a page instead of a redirect (session probably expired)"
                ));
            }
            return Err(anyhow!("SSO authorize returned status {status} without a redirect"));
        };
        let absolute = match url::Url::parse(&location) {
            Ok(u) => u.to_string(),
            Err(_) => url::Url::parse(&next)
                .and_then(|base| base.join(&location))
                .map(|u| u.to_string())
                .unwrap_or(location),
        };
        if let Some(c) = code_from_redirect(&absolute, &pkce.state) {
            code = Some(c);
            break;
        }
        if absolute.contains("/login") {
            return Err(anyhow!("SSO session expired; sign in again"));
        }
        next = absolute;
    }
    let code = code.ok_or_else(|| anyhow!("SSO never redirected back with an authorization code"))?;

    let resp = client
        .post(format!("{AUTHORITY}/accessToken"))
        .header("Origin", "https://consumer.hotmart.com")
        .header("Referer", "https://consumer.hotmart.com/")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CONSUMER_CLIENT_ID),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .send()
        .await?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let err = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(anyhow!("token exchange returned status {status} {err}"));
    }
    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("token exchange answered without access_token"))?
        .to_string();
    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .map(|secs| now_unix() + secs);
    Ok(OidcUser {
        access_token,
        expires_at,
        email: None,
        name: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oidc_client_user_entry() {
        let raw = r#"{"id_token":"x","session_state":"s","access_token":"AT-1-abc","token_type":"bearer","scope":"openid","profile":{"sub":"1","email":"me@example.com","name":"Me"},"expires_at":1789785134}"#;
        let u = parse_user_entry(raw).unwrap();
        assert_eq!(u.access_token, "AT-1-abc");
        assert_eq!(u.expires_at, Some(1789785134));
        assert_eq!(u.email.as_deref(), Some("me@example.com"));
        assert_eq!(u.name.as_deref(), Some("Me"));
        assert!(parse_user_entry("{}").is_none());
        assert!(parse_user_entry("not json").is_none());
    }

    #[test]
    fn finds_entry_among_webview_pairs() {
        let pairs = vec![
            ("hmLangCookie".to_string(), "pt".to_string()),
            (
                "oidc.user:https://sso.hotmart.com/oidc:0fff6c2a".to_string(),
                r#"{"access_token":"AT-2","expires_at":5}"#.to_string(),
            ),
        ];
        assert_eq!(find_user_entry(&pairs).unwrap().access_token, "AT-2");
        let ss = vec![(
            "ss:oidc.user:https://sso.hotmart.com/oidc:0fff6c2a".to_string(),
            r#"{"access_token":"AT-3"}"#.to_string(),
        )];
        assert_eq!(find_user_entry(&ss).unwrap().access_token, "AT-3");
        assert!(find_user_entry(&[("token".into(), "AT-4".into())]).is_none());
    }

    #[test]
    fn userinfo_profile_reads_nested_attributes() {
        let body = serde_json::json!({
            "sub": "1", "service": "x", "id": "abc",
            "attributes": {"email": "a@b.c", "name": "A B"}
        });
        let p = profile_from_userinfo(&body);
        assert_eq!(p.email.as_deref(), Some("a@b.c"));
        assert_eq!(p.name.as_deref(), Some("A B"));
        assert_eq!(p.id.as_deref(), Some("abc"));
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = new_pkce();
        assert!(p.verifier.len() >= 43);
        assert_eq!(p.challenge, b64url(&Sha256::digest(p.verifier.as_bytes())));
        let url = authorize_url(&p);
        assert!(url.starts_with("https://sso.hotmart.com/oidc/authorize?client_id="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={}", p.state)));
    }

    #[test]
    fn redirect_code_requires_consumer_host_and_state() {
        assert_eq!(
            code_from_redirect("https://consumer.hotmart.com/auth/login?code=OC-9&state=st", "st").as_deref(),
            Some("OC-9")
        );
        assert!(code_from_redirect("https://consumer.hotmart.com/auth/login?code=OC-9&state=other", "st").is_none());
        assert!(code_from_redirect("https://evil.example/auth/login?code=OC-9&state=st", "st").is_none());
        assert!(code_from_redirect("https://sso.hotmart.com/login?service=x", "st").is_none());
    }
}

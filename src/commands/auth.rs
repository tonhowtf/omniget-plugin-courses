use std::time::{Duration, Instant};

use crate::platforms::hotmart::auth::{
    authenticate, delete_saved_session, load_saved_session, save_session, HotmartSession,
    SavedSession,
};
use crate::platforms::hotmart::oidc;

const SESSION_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Legacy cookie names that carried a bearer token before Hotmart moved the
/// consumer area to OIDC. Still honoured for sessions captured the old way.
const LEGACY_TOKEN_COOKIES: [&str; 5] = [
    "hmvlcintegration",
    "access_token",
    "hotmart.token",
    "hotmart-token",
    "token",
];

async fn reset_state(plugin: &crate::CoursesPlugin) {
    let _ = delete_saved_session().await;
    {
        let mut map = plugin.active_downloads.lock().await;
        for token in map.values() {
            token.cancel();
        }
        map.clear();
    }
    plugin.hotmart_session.lock().await.take();
    *plugin.session_validated_at.lock().await = None;
    *plugin.courses_cache.lock().await = None;
}

async fn install_session(plugin: &crate::CoursesPlugin, session: HotmartSession) -> String {
    let email = session.email.clone();
    let _ = save_session(&session).await;
    *plugin.hotmart_session.lock().await = Some(session);
    *plugin.session_validated_at.lock().await = Some(Instant::now());
    *plugin.courses_cache.lock().await = None;
    email
}

pub async fn hotmart_login(
    host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    plugin: &crate::CoursesPlugin,
    email: String,
    password: String,
) -> Result<String, String> {
    reset_state(plugin).await;

    match authenticate(&host, &email, &password).await {
        Ok(session) => Ok(install_session(plugin, session).await),
        Err(e) => {
            tracing::error!("Hotmart login failed: {}", e);
            Err(format!("Login failed: {}", e))
        }
    }
}

/// Which endpoint accepted the token, so callers can log it.
#[derive(Debug, PartialEq)]
pub enum TokenKind {
    Oidc,
    Legacy,
}

/// Validates a bearer token: first against the OIDC userinfo endpoint (the
/// tokens the consumer app issues today), then against the legacy
/// `check_token` (tokens from the old `hmVlcIntegration` cookie).
pub async fn validate_token(
    client: &reqwest::Client,
    token: &str,
) -> anyhow::Result<(TokenKind, oidc::Profile)> {
    match oidc::fetch_profile(client, token).await {
        Ok(profile) => return Ok((TokenKind::Oidc, profile)),
        Err(e) => tracing::info!("[hotmart] userinfo rejected the token ({}), trying check_token", e),
    }

    let resp = client
        .post("https://api-sec-vlc.hotmart.com/security/oauth/check_token")
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("check_token request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "token rejected by userinfo and by check_token (status {status})"
        ));
    }
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let email = body
        .get("user_name")
        .or_else(|| body.get("email"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok((
        TokenKind::Legacy,
        oidc::Profile {
            email,
            name: None,
            id: None,
        },
    ))
}

/// Mints a new token from the saved SSO cookies and swaps it into the session.
async fn renew_session(session: &HotmartSession) -> anyhow::Result<HotmartSession> {
    let user = oidc::exchange_with_cookies(&session.cookies).await?;
    let renewed = session.with_token(user.access_token, user.expires_at)?;
    tracing::info!(
        "[hotmart] token renewed through SSO cookies (expires_at={:?})",
        renewed.expires_at
    );
    Ok(renewed)
}

pub async fn hotmart_check_session(plugin: &crate::CoursesPlugin) -> Result<String, String> {
    tracing::info!("[hotmart] check_session: start");
    let has_memory_session = plugin.hotmart_session.lock().await.is_some();

    if !has_memory_session {
        tracing::info!("[hotmart] check_session: no memory session, loading from disk");
        match load_saved_session().await {
            Ok(session) => {
                tracing::info!("[hotmart] check_session: loaded session for {}", session.email);
                *plugin.hotmart_session.lock().await = Some(session);
            }
            Err(e) => {
                tracing::info!("[hotmart] check_session: no saved session: {}", e);
                return Err("not_authenticated".to_string());
            }
        }
    }

    let session = {
        let guard = plugin.hotmart_session.lock().await;
        guard
            .as_ref()
            .ok_or_else(|| "not_authenticated".to_string())?
            .clone()
    };
    let email = session.email.clone();

    if !session.is_expired() {
        let validated_at = plugin.session_validated_at.lock().await;
        if let Some(at) = *validated_at {
            if at.elapsed() < SESSION_COOLDOWN {
                tracing::info!("[hotmart] check_session: cooldown active, returning {}", email);
                return Ok(email);
            }
        }
    }

    if session.is_expired() {
        tracing::info!("[hotmart] check_session: token expired, renewing through SSO");
        return match renew_session(&session).await {
            Ok(renewed) => Ok(install_session(plugin, renewed).await),
            Err(e) => {
                tracing::warn!("[hotmart] check_session: renewal failed: {}", e);
                reset_state(plugin).await;
                Err("session_expired".to_string())
            }
        };
    }

    tracing::info!("[hotmart] check_session: validating token (len={})", session.token.len());
    match validate_token(&session.client, &session.token).await {
        Ok((kind, profile)) => {
            tracing::info!("[hotmart] check_session: token accepted ({:?})", kind);
            *plugin.session_validated_at.lock().await = Some(Instant::now());
            if email.is_empty() || email == "hotmart_user" {
                if let Some(found) = profile.email {
                    let mut guard = plugin.hotmart_session.lock().await;
                    if let Some(s) = guard.as_mut() {
                        s.email = found.clone();
                        let _ = save_session(s).await;
                    }
                    return Ok(found);
                }
            }
            Ok(email)
        }
        Err(e) => {
            tracing::warn!("[hotmart] check_session: {}; trying to renew through SSO", e);
            match renew_session(&session).await {
                Ok(renewed) => Ok(install_session(plugin, renewed).await),
                Err(renew_err) => {
                    tracing::warn!("[hotmart] check_session: renewal failed: {}", renew_err);
                    if e.to_string().contains("request failed") {
                        return Err(format!("Validation error: {}", e));
                    }
                    reset_state(plugin).await;
                    Err("session_expired".to_string())
                }
            }
        }
    }
}

pub async fn hotmart_logout(plugin: &crate::CoursesPlugin) -> Result<(), String> {
    reset_state(plugin).await;
    Ok(())
}

/// One captured cookie or web-storage entry. `domain`/`path` are accepted and
/// ignored: everything is re-scoped to `.hotmart.com` when the client is built.
#[derive(serde::Deserialize)]
struct CookieEntry {
    name: String,
    value: String,
}

/// Accepts the JSON array the auth webview produces, a JSON object of
/// name→value, or a Netscape `cookies.txt` export (browser extensions and the
/// Cookie Manager hand those over).
pub fn parse_cookie_input(input: &str) -> Result<Vec<(String, String)>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        let entries: Vec<CookieEntry> =
            serde_json::from_str(trimmed).map_err(|e| format!("Invalid cookies JSON: {}", e))?;
        return Ok(entries.into_iter().map(|c| (c.name, c.value)).collect());
    }
    if trimmed.starts_with('{') {
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(trimmed).map_err(|e| format!("Invalid cookies JSON: {}", e))?;
        return Ok(map
            .into_iter()
            .filter_map(|(k, v)| match v {
                serde_json::Value::String(s) => Some((k, s)),
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    Some((k, v.to_string()))
                }
                _ => None,
            })
            .collect());
    }
    let mut pairs = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 7 {
            pairs.push((cols[5].to_string(), cols[6].to_string()));
        } else if let Some((k, v)) = line.split_once('=') {
            pairs.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    if pairs.is_empty() {
        return Err("Could not read any cookie from the pasted text (expected JSON or a Netscape cookies.txt export)".into());
    }
    Ok(pairs)
}

fn legacy_token(pairs: &[(String, String)]) -> Option<(String, String)> {
    LEGACY_TOKEN_COOKIES
        .iter()
        .find_map(|wanted| {
            pairs
                .iter()
                .find(|(n, _)| n.to_lowercase() == *wanted)
                .map(|(n, v)| (n.clone(), v.clone()))
        })
        .or_else(|| {
            pairs
                .iter()
                .find(|(name, value)| {
                    let n = name.to_lowercase();
                    !n.contains(':')
                        && (n.contains("token") || n.contains("access"))
                        && (value.matches('.').count() == 2 || value.len() > 20)
                })
                .map(|(n, v)| (n.clone(), v.clone()))
        })
}

pub async fn hotmart_set_cookies(
    plugin: &crate::CoursesPlugin,
    cookies_json: String,
) -> Result<String, String> {
    reset_state(plugin).await;

    let cookie_pairs = parse_cookie_input(&cookies_json)?;
    tracing::info!(
        "[hotmart] set_cookies: received {} entries, input size={}",
        cookie_pairs.len(),
        cookies_json.len()
    );
    if cookie_pairs.is_empty() {
        return Err("No cookies provided".to_string());
    }
    let names: Vec<&str> = cookie_pairs.iter().map(|(n, _)| n.as_str()).collect();
    tracing::info!("[hotmart] entry names: {:?}", names);

    let mut email_hint = None;
    let (token, expires_at) = if let Some(user) = oidc::find_user_entry(&cookie_pairs) {
        tracing::info!("[hotmart] using the OIDC session from web storage");
        email_hint = user.email.clone();
        (user.access_token, user.expires_at)
    } else if let Some((name, value)) = legacy_token(&cookie_pairs) {
        tracing::info!("[hotmart] using token from cookie '{}'", name);
        (value, None)
    } else {
        tracing::info!("[hotmart] no token among the entries; running the SSO code exchange");
        match oidc::exchange_with_cookies(&cookie_pairs).await {
            Ok(user) => (user.access_token, user.expires_at),
            Err(e) => {
                return Err(format!(
                    "No Hotmart session found in the cookies and the SSO sign-in could not be reused ({}). Sign in with the browser button, or export the cookies while logged in at consumer.hotmart.com.",
                    e
                ))
            }
        }
    };
    tracing::info!("[hotmart] token found: len={}, expires_at={:?}", token.len(), expires_at);

    let saved = SavedSession {
        token,
        email: String::new(),
        cookies: cookie_pairs,
        saved_at: oidc::now_unix(),
        expires_at,
    };
    let session = HotmartSession::from_saved(saved).map_err(|e| format!("Failed to build client: {}", e))?;

    let (kind, profile) = validate_token(&session.client, &session.token)
        .await
        .map_err(|e| format!("Token validation failed: {} - the session may be expired", e))?;
    tracing::info!("[hotmart] token validation: accepted as {:?}", kind);

    let mut session = session;
    session.email = profile
        .email
        .or(email_hint)
        .or(profile.name)
        .unwrap_or_else(|| "hotmart_user".to_string());

    let email = install_session(plugin, session).await;
    tracing::info!("[hotmart] browser login successful for {}", email);
    Ok(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_webview_json_array() {
        let pairs = parse_cookie_input(r#"[{"name":"a","value":"1","domain":".hotmart.com","path":"/"},{"name":"b","value":"2"}]"#).unwrap();
        assert_eq!(pairs, vec![("a".into(), "1".into()), ("b".into(), "2".into())]);
    }

    #[test]
    fn parses_netscape_export_and_key_value_lines() {
        let txt = "# Netscape HTTP Cookie File\n.hotmart.com\tTRUE\t/\tTRUE\t0\tTGC\tabc\n\nfoo=bar\n";
        let pairs = parse_cookie_input(txt).unwrap();
        assert_eq!(pairs, vec![("TGC".into(), "abc".into()), ("foo".into(), "bar".into())]);
        assert!(parse_cookie_input("just words").is_err());
        assert!(parse_cookie_input("   ").unwrap().is_empty());
    }

    #[test]
    fn parses_object_form_and_keeps_nested_values_as_json() {
        let pairs = parse_cookie_input(r#"{"TGC":"abc","oidc.user:x":{"access_token":"AT"}}"#).unwrap();
        assert_eq!(pairs[0], ("TGC".into(), "abc".into()));
        assert_eq!(pairs[1].0, "oidc.user:x");
        assert!(pairs[1].1.contains("access_token"));
    }

    #[test]
    fn legacy_token_prefers_named_cookies_and_skips_storage_keys() {
        let pairs = vec![
            ("oidc.user:x".to_string(), "not-a-token-value-but-long-enough".to_string()),
            ("csrftoken".to_string(), "short".to_string()),
            ("hmVlcIntegration".to_string(), "legacy".to_string()),
        ];
        assert_eq!(legacy_token(&pairs).unwrap().0, "hmVlcIntegration");
        let only_storage = vec![("oidc.user:x".to_string(), "a.b.c".to_string())];
        assert!(legacy_token(&only_storage).is_none());
    }
}

use std::time::{Duration, Instant};

use crate::platforms::hotmart::auth::{
    authenticate, build_client_from_saved, delete_saved_session, load_saved_session, save_session,
    HotmartSession, SavedSession,
};


const SESSION_COOLDOWN: Duration = Duration::from_secs(5 * 60);


pub async fn hotmart_login(
    host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    plugin: &crate::CoursesPlugin,
    email: String,
    password: String,
) -> Result<String, String> {
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

    match authenticate(&host, &email, &password).await {
        Ok(session) => {
            let response_email = session.email.clone();
            let _ = save_session(&session).await;
            let mut guard = plugin.hotmart_session.lock().await;
            *guard = Some(session);
            *plugin.session_validated_at.lock().await = Some(Instant::now());
            Ok(response_email)
        }
        Err(e) => {
            tracing::error!("Hotmart login failed: {}", e);
            Err(format!("Login failed: {}", e))
        }
    }
}


pub async fn hotmart_check_session(
    plugin: &crate::CoursesPlugin,
) -> Result<String, String> {
    let has_memory_session = plugin.hotmart_session.lock().await.is_some();

    if !has_memory_session {
        match load_saved_session().await {
            Ok(session) => {
                let mut guard = plugin.hotmart_session.lock().await;
                *guard = Some(session);
            }
            Err(_) => {
                return Err("not_authenticated".to_string());
            }
        }
    }

    let guard = plugin.hotmart_session.lock().await;
    let session = guard
        .as_ref()
        .ok_or_else(|| "not_authenticated".to_string())?;
    let email = session.email.clone();

    {
        let validated_at = plugin.session_validated_at.lock().await;
        if let Some(at) = *validated_at {
            if at.elapsed() < SESSION_COOLDOWN {
                return Ok(email);
            }
        }
    }

    let token = session.token.clone();
    let client = session.client.clone();
    drop(guard);

    let resp = client
        .post("https://api-sec-vlc.hotmart.com/security/oauth/check_token")
        .form(&[("token", &token)])
        .send()
        .await
        .map_err(|e| format!("Validation error: {}", e))?;

    if resp.status().is_success() {
        *plugin.session_validated_at.lock().await = Some(Instant::now());
        Ok(email)
    } else {
        plugin.hotmart_session.lock().await.take();
        *plugin.session_validated_at.lock().await = None;
        *plugin.courses_cache.lock().await = None;
        let _ = delete_saved_session().await;
        Err("session_expired".to_string())
    }
}


pub async fn hotmart_logout(
    plugin: &crate::CoursesPlugin,
) -> Result<(), String> {
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
    Ok(())
}


pub async fn hotmart_set_cookies(
    plugin: &crate::CoursesPlugin,
    cookies_json: String,
) -> Result<String, String> {
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

    #[derive(serde::Deserialize)]
    struct CookieEntry {
        name: String,
        value: String,
        #[allow(dead_code)]
        domain: Option<String>,
        #[allow(dead_code)]
        path: Option<String>,
    }

    let cookies: Vec<CookieEntry> = serde_json::from_str(&cookies_json)
        .map_err(|e| format!("Invalid cookies JSON: {}", e))?;

    tracing::info!("[hotmart] set_cookies: received {} cookies, json size={}", cookies.len(), cookies_json.len());

    if cookies.is_empty() {
        return Err("No cookies provided".to_string());
    }

    let cookie_pairs: Vec<(String, String)> = cookies
        .iter()
        .map(|c| (c.name.clone(), c.value.clone()))
        .collect();

    let cookie_names: Vec<&str> = cookie_pairs.iter().map(|(n, _)| n.as_str()).collect();
    tracing::info!("[hotmart] cookie names: {:?}", cookie_names);

    let token = cookie_pairs
        .iter()
        .find(|(name, _)| {
            let n = name.to_lowercase();
            n == "hmvlcintegration"
        })
        .or_else(|| {
            cookie_pairs.iter().find(|(name, _)| {
                let n = name.to_lowercase();
                n == "access_token"
                    || n == "hotmart.token"
                    || n == "hotmart-token"
                    || n == "token"
            })
        })
        .or_else(|| {
            cookie_pairs.iter().find(|(name, value)| {
                let n = name.to_lowercase();
                (n.contains("token") || n.contains("access"))
                    && (value.matches('.').count() == 2 || value.len() > 20)
            })
        })
        .map(|(name, value)| {
            tracing::info!("[hotmart] using token from cookie '{}'", name);
            value.clone()
        })
        .ok_or_else(|| {
            format!(
                "No auth token found in cookies. Received cookie names: {:?}",
                cookie_names
            )
        })?;

    tracing::info!(
        "[hotmart] token found: len={}, starts_with={}",
        token.len(),
        &token[..token.len().min(10)]
    );

    let saved = SavedSession {
        token: token.clone(),
        email: String::new(),
        cookies: cookie_pairs.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    let client = build_client_from_saved(&saved)
        .map_err(|e| format!("Failed to build client: {}", e))?;

    let resp = client
        .post("https://api-sec-vlc.hotmart.com/security/oauth/check_token")
        .form(&[("token", &token)])
        .send()
        .await
        .map_err(|e| format!("Token validation failed: {}", e))?;

    let status = resp.status();
    tracing::info!("[hotmart] token validation response: {}", status);

    if !status.is_success() {
        return Err(format!("Token validation failed (status {}) - cookies may be expired", status));
    }

    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let email = body
        .get("user_name")
        .or_else(|| body.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or("hotmart_user")
        .to_string();

    let session = HotmartSession {
        token,
        email: email.clone(),
        client,
        cookies: cookie_pairs,
    };

    let _ = save_session(&session).await;
    *plugin.hotmart_session.lock().await = Some(session);
    *plugin.session_validated_at.lock().await = Some(Instant::now());

    tracing::info!("[hotmart] browser login successful for {}", email);
    Ok(email)
}

use std::time::{Duration, Instant};

use crate::platforms::hotmart::auth::{authenticate, delete_saved_session, load_saved_session, save_session};


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

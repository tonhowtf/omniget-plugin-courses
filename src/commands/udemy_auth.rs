use std::time::{Duration, Instant};

use crate::platforms::udemy::auth::{
    authenticate, authenticate_with_cookie_json, delete_saved_session, load_saved_session,
    request_otp, save_session, verify_otp,
};


const SESSION_COOLDOWN: Duration = Duration::from_secs(5 * 60);


pub async fn udemy_request_otp(
    email: String,
) -> Result<(), String> {
    request_otp(&email)
        .await
        .map_err(|e| format!("OTP request failed: {}", e))
}


pub async fn udemy_verify_otp(
    plugin: &crate::CoursesPlugin,
    email: String,
    otp_code: String,
) -> Result<String, String> {
    let _ = delete_saved_session().await;
    plugin.udemy_session.lock().await.take();
    *plugin.udemy_session_validated_at.lock().await = None;
    *plugin.udemy_courses_cache.lock().await = None;

    match verify_otp(&email, &otp_code).await {
        Ok(session) => {
            let response_email = session.email.clone();
            let _ = save_session(&session).await;
            let mut guard = plugin.udemy_session.lock().await;
            *guard = Some(session);
            *plugin.udemy_session_validated_at.lock().await = Some(Instant::now());
            Ok(response_email)
        }
        Err(e) => {
            tracing::error!("[udemy] OTP verify failed: {}", e);
            Err(format!("OTP verification failed: {}", e))
        }
    }
}


pub async fn udemy_login(
    _host: std::sync::Arc<dyn omniget_plugin_sdk::PluginHost>,
    _plugin: &crate::CoursesPlugin,
    _email: String,
) -> Result<String, String> {
    Err("Please use Browser Login for Udemy. The email login is not supported due to Cloudflare protection.".to_string())
}


pub async fn udemy_login_cookies(
    plugin: &crate::CoursesPlugin,
    cookie_json: String,
) -> Result<String, String> {
    let _ = delete_saved_session().await;
    plugin.udemy_session.lock().await.take();
    *plugin.udemy_session_validated_at.lock().await = None;
    *plugin.udemy_courses_cache.lock().await = None;

    match authenticate_with_cookie_json(&cookie_json).await {
        Ok(session) => {
            let response_email = session.email.clone();
            let _ = save_session(&session).await;
            let mut guard = plugin.udemy_session.lock().await;
            *guard = Some(session);
            *plugin.udemy_session_validated_at.lock().await = Some(Instant::now());
            Ok(response_email)
        }
        Err(e) => {
            tracing::error!("[udemy] cookie login failed: {}", e);
            Err(format!("Cookie login failed: {}", e))
        }
    }
}


pub async fn udemy_check_session(
    plugin: &crate::CoursesPlugin,
) -> Result<String, String> {
    let has_memory_session = plugin.udemy_session.lock().await.is_some();

    if !has_memory_session {
        match load_saved_session().await {
            Ok(session) => {
                let mut guard = plugin.udemy_session.lock().await;
                *guard = Some(session);
            }
            Err(_) => {
                return Err("not_authenticated".to_string());
            }
        }
    }

    let guard = plugin.udemy_session.lock().await;
    let session = guard
        .as_ref()
        .ok_or_else(|| "not_authenticated".to_string())?;
    let email = session.email.clone();

    {
        let validated_at = plugin.udemy_session_validated_at.lock().await;
        if let Some(at) = *validated_at {
            if at.elapsed() < SESSION_COOLDOWN {
                return Ok(email);
            }
        }
    }

    let client = session.client.clone();
    let portal = session.portal_name.clone();
    drop(guard);

    let validation_url = format!("https://{}.udemy.com/api-2.0/users/me/", portal);
    let resp = client
        .get(&validation_url)
        .send()
        .await
        .map_err(|e| format!("Validation error: {}", e))?;

    let status = resp.status();
    if status.is_success() {
        *plugin.udemy_session_validated_at.lock().await = Some(Instant::now());
        Ok(email)
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        plugin.udemy_session.lock().await.take();
        *plugin.udemy_session_validated_at.lock().await = None;
        *plugin.udemy_courses_cache.lock().await = None;
        let _ = delete_saved_session().await;
        Err("session_expired".to_string())
    } else if status == reqwest::StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        let body_lower = body.to_lowercase();
        if body_lower.contains("just a moment") || body_lower.contains("cf_chl") || body_lower.contains("<!doctype") {
            tracing::warn!("[udemy] check_session got Cloudflare challenge, keeping session alive");
            *plugin.udemy_session_validated_at.lock().await = Some(Instant::now());
            Ok(email)
        } else {
            tracing::error!("[udemy] check_session got real 403: {}", &body[..body.len().min(300)]);
            plugin.udemy_session.lock().await.take();
            *plugin.udemy_session_validated_at.lock().await = None;
            *plugin.udemy_courses_cache.lock().await = None;
            let _ = delete_saved_session().await;
            Err("session_expired".to_string())
        }
    } else {
        Err(format!("session_check_failed: {}", status))
    }
}


pub async fn udemy_get_portal(
    plugin: &crate::CoursesPlugin,
) -> Result<String, String> {
    let guard = plugin.udemy_session.lock().await;
    Ok(guard
        .as_ref()
        .map(|s| s.portal_name.clone())
        .unwrap_or_else(|| "www".into()))
}


pub async fn udemy_logout(
    plugin: &crate::CoursesPlugin,
) -> Result<(), String> {
    let _ = delete_saved_session().await;
    plugin.udemy_session.lock().await.take();
    *plugin.udemy_session_validated_at.lock().await = None;
    *plugin.udemy_courses_cache.lock().await = None;
    plugin.udemy_api_webview.lock().await.take();
    Ok(())
}


pub async fn udemy_set_cookies(
    plugin: &crate::CoursesPlugin,
    cookies_json: String,
) -> Result<String, String> {
    let _ = delete_saved_session().await;
    plugin.udemy_session.lock().await.take();
    *plugin.udemy_session_validated_at.lock().await = None;
    *plugin.udemy_courses_cache.lock().await = None;

    match authenticate_with_cookie_json(&cookies_json).await {
        Ok(session) => {
            let email = session.email.clone();
            let _ = save_session(&session).await;
            *plugin.udemy_session.lock().await = Some(session);
            *plugin.udemy_session_validated_at.lock().await = Some(Instant::now());
            tracing::info!("[udemy] browser login successful for {}", email);
            Ok(email)
        }
        Err(e) => {
            tracing::error!("[udemy] browser cookie login failed: {}", e);
            Err(format!("Cookie login failed: {}", e))
        }
    }
}

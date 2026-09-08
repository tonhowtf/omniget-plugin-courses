use std::time::Instant;

use crate::platforms::udemy::api::UdemyCourse;
use crate::state::UdemyCoursesCache;

const COURSES_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

#[allow(dead_code)]
async fn get_portal(plugin: &crate::CoursesPlugin) -> String {
    let guard = plugin.udemy_session.lock().await;
    guard
        .as_ref()
        .map(|s| s.portal_name.clone())
        .unwrap_or_else(|| "www".into())
}

async fn fetch_courses_via_api(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<UdemyCourse>, String> {
    let session = {
        let guard = plugin.udemy_session.lock().await;
        guard.clone().ok_or("not_authenticated")?
    };

    tracing::info!(
        "[udemy-api] fetching courses: portal={}, token_len={}",
        session.portal_name,
        session.access_token.len()
    );

    let courses = crate::platforms::udemy::api::list_all_courses(&session, &session.portal_name)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("[udemy-api] found {} total courses via direct API", courses.len());

    let mut cache = plugin.udemy_courses_cache.lock().await;
    *cache = Some(UdemyCoursesCache {
        courses: courses.clone(),
        fetched_at: Instant::now(),
    });

    Ok(courses)
}

async fn fetch_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<UdemyCourse>, String> {
    fetch_courses_via_api(plugin).await
}


pub async fn udemy_list_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<UdemyCourse>, String> {
    {
        let cache = plugin.udemy_courses_cache.lock().await;
        if let Some(ref cached) = *cache {
            if cached.fetched_at.elapsed() < COURSES_CACHE_TTL {
                return Ok(cached.courses.clone());
            }
        }
    }

    fetch_courses(plugin).await
}


pub async fn udemy_refresh_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<UdemyCourse>, String> {
    {
        let mut cache = plugin.udemy_courses_cache.lock().await;
        *cache = None;
    }
    fetch_courses(plugin).await
}

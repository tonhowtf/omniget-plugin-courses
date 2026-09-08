use std::time::{Duration, Instant};

use crate::platforms::hotmart::api::{self, Course, Module};
use crate::platforms::hotmart::course_list;
use crate::state::CoursesCache;

const COURSES_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

async fn fetch_courses_from_api(plugin: &crate::CoursesPlugin) -> Result<Vec<Course>, String> {
    let guard = plugin.hotmart_session.lock().await;
    let session = guard
        .as_ref()
        .ok_or_else(|| "Not authenticated. Please log in first.".to_string())?;

    let subdomains = match api::get_subdomains(session).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("[hotmart] check_token resources unavailable, purchases only: {}", e);
            Vec::new()
        }
    };

    let mut courses = api::list_courses(session).await.map_err(|e| e.to_string())?;
    let from_purchases = courses.len();

    let from_free = match api::list_free_courses(session).await {
        Ok(free) => {
            let mut added = 0;
            for c in free {
                if !courses.iter().any(|existing| existing.id == c.id) {
                    courses.push(c);
                    added += 1;
                }
            }
            added
        }
        Err(e) => {
            tracing::warn!("[hotmart] free products unavailable: {}", e);
            0
        }
    };

    for course in &mut courses {
        match api::get_course_details(session, course.id).await {
            Ok(details) => {
                if details.price.is_some() {
                    course.price = details.price;
                }
                if course.slug.is_none() {
                    course.slug = details.slug;
                }
                if course.external_url.is_none() {
                    course.external_url = details.external_url;
                }
            }
            Err(e) => tracing::debug!("[hotmart] details for {}: {}", course.id, e),
        }
    }

    let from_club_access = course_list::append_club_only_courses(&mut courses, &subdomains);
    api::merge_subdomains(&mut courses, &subdomains);
    let external = courses.iter().filter(|c| c.external_platform).count();

    tracing::info!(
        "[hotmart] courses: {} from purchases, {} free, {} club resources ({} added as club-only), {} total, {} external",
        from_purchases,
        from_free,
        subdomains.len(),
        from_club_access,
        courses.len(),
        external
    );

    let mut cache = plugin.courses_cache.lock().await;
    *cache = Some(CoursesCache {
        courses: courses.clone(),
        fetched_at: Instant::now(),
    });

    Ok(courses)
}


pub async fn hotmart_list_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<Course>, String> {
    {
        let cache = plugin.courses_cache.lock().await;
        if let Some(ref cached) = *cache {
            if cached.fetched_at.elapsed() < COURSES_CACHE_TTL {
                return Ok(cached.courses.clone());
            }
        }
    }

    fetch_courses_from_api(&plugin).await
}


pub async fn hotmart_refresh_courses(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<Course>, String> {
    {
        let mut cache = plugin.courses_cache.lock().await;
        *cache = None;
    }
    fetch_courses_from_api(&plugin).await
}


pub async fn hotmart_get_modules(
    plugin: &crate::CoursesPlugin,
    course_id: u64,
    slug: String,
) -> Result<Vec<Module>, String> {
    let guard = plugin.hotmart_session.lock().await;
    let session = guard
        .as_ref()
        .ok_or_else(|| "Not authenticated. Please log in first.".to_string())?;

    api::get_modules(session, &slug, course_id)
        .await
        .map_err(|e| e.to_string())
}

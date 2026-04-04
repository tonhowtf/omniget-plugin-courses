use std::time::Instant;

use crate::platforms::udemy::api::UdemyCourse;
use crate::state::UdemyCoursesCache;

const COURSES_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

async fn get_portal(plugin: &crate::CoursesPlugin) -> String {
    let guard = plugin.udemy_session.lock().await;
    guard
        .as_ref()
        .map(|s| s.portal_name.clone())
        .unwrap_or_else(|| "www".into())
}

fn parse_courses_from_results(results: &[serde_json::Value]) -> Vec<UdemyCourse> {
    results
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_u64()?;
            let title = item.get("title")?.as_str().unwrap_or("").to_string();
            let published_title = item
                .get("published_title")?
                .as_str()
                .unwrap_or("")
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let image_url = item
                .get("image_240x135")
                .or_else(|| item.get("image_480x270"))
                .or_else(|| item.get("image_url"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let num_published_lectures = item
                .get("num_published_lectures")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32);

            Some(UdemyCourse {
                id,
                title,
                published_title,
                url,
                image_url,
                num_published_lectures,
            })
        })
        .collect()
}

async fn fetch_courses_via_api(
    plugin: &crate::CoursesPlugin,
) -> Result<Vec<UdemyCourse>, String> {
    let (client, portal) = {
        let guard = plugin.udemy_session.lock().await;
        let session = guard.as_ref().ok_or("not_authenticated")?;
        (session.client.clone(), session.portal_name.clone())
    };

    let url = format!(
        "https://{}.udemy.com/api-2.0/users/me/subscribed-courses?fields[course]=id,url,title,published_title,image_240x135,num_published_lectures&ordering=-last_accessed,-access_time&page=1&page_size=10000",
        portal
    );

    let token_len = {
        let guard = plugin.udemy_session.lock().await;
        guard.as_ref().map(|s| s.access_token.len()).unwrap_or(0)
    };
    tracing::info!("[udemy-api] fetching courses: portal={}, token_len={}", portal, token_len);

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("API request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        tracing::error!("[udemy-api] list_courses failed: status={}, body={}", status, &body[..body.len().min(500)]);
        return Err(format!("API returned status {}", status));
    }

    let body = resp.text().await.map_err(|e| format!("Read body failed: {}", e))?;
    let data: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("JSON parse error: {}", e))?;

    let results = data
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let mut courses = parse_courses_from_results(&results);

    let sub_url = format!(
        "https://{}.udemy.com/api-2.0/users/me/subscription-course-enrollments?fields[course]=id,title,published_title,image_240x135,num_published_lectures&page=1&page_size=50",
        portal
    );

    if let Ok(sub_resp) = client.get(&sub_url).send().await {
        if sub_resp.status().is_success() {
            if let Ok(sub_body) = sub_resp.text().await {
                if let Ok(sub_data) = serde_json::from_str::<serde_json::Value>(&sub_body) {
                    if let Some(sub_results) = sub_data.get("results").and_then(|r| r.as_array()) {
                        let existing_ids: std::collections::HashSet<u64> =
                            courses.iter().map(|c| c.id).collect();
                        for c in parse_courses_from_results(sub_results) {
                            if !existing_ids.contains(&c.id) {
                                courses.push(c);
                            }
                        }
                    }
                }
            }
        }
    }

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

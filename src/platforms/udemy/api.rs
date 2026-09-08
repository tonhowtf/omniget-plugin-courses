use std::time::Duration;

use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::auth::UdemySession;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdemyCourse {
    pub id: u64,
    pub title: String,
    pub published_title: String,
    pub url: Option<String>,
    pub image_url: Option<String>,
    pub num_published_lectures: Option<u32>,
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdemyChapter {
    pub id: u64,
    pub title: String,
    pub object_index: u32,
    pub lectures: Vec<UdemyLecture>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdemyLecture {
    pub id: u64,
    pub title: String,
    pub object_index: u32,
    pub lecture_class: String,
    pub asset: Option<serde_json::Value>,
    #[serde(default)]
    pub supplementary_assets: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdemyCurriculum {
    pub course_id: u64,
    pub title: String,
    pub chapters: Vec<UdemyChapter>,
    pub total_lectures: u32,
    pub total_video_lectures: u32,
    pub drm_video_lectures: u32,
}

pub fn parse_chapter_filter(s: &str) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                for n in start..=end {
                    out.insert(n);
                }
            }
        } else if let Ok(n) = part.parse::<u32>() {
            out.insert(n);
        }
    }
    out
}

pub fn extract_course_name(url: &str) -> Option<(String, String)> {
    let re_learn = Regex::new(
        r"(?i)://([^/]+?)\.udemy\.com/course/([a-zA-Z0-9_-]+)/learn"
    ).ok()?;
    if let Some(caps) = re_learn.captures(url) {
        let portal_name = caps.get(1)?.as_str().to_string();
        let course_slug = caps.get(2)?.as_str().to_string();
        return Some((portal_name, course_slug));
    }

    let re = Regex::new(
        r"(?i)://(.+?)\.udemy\.com/(?:course(?:/draft)*/)?([a-zA-Z0-9_-]+)"
    ).ok()?;
    let caps = re.captures(url)?;
    let portal_name = caps.get(1)?.as_str().to_string();
    let course_slug = caps.get(2)?.as_str().to_string();
    Some((portal_name, course_slug))
}

async fn api_get_with_retry(
    client: &reqwest::Client,
    url: &str,
    params: Option<&[(&str, &str)]>,
) -> Result<reqwest::Response> {
    let max_attempts: u32 = 3;
    let mut last_err = None;

    for attempt in 0..max_attempts {
        let mut req = client.get(url);
        if let Some(p) = params {
            req = req.query(p);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                tracing::info!("[udemy-api] {} → {}", url, status);
                if status.is_success() {
                    return Ok(resp);
                }
                if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                    let body = resp.text().await.unwrap_or_default();
                    tracing::error!("[udemy-api] auth error for {}: {} — {}", url, status, &body[..body.len().min(500)]);
                    return Err(anyhow!("API auth error ({}): {}", status, body));
                }
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!("[udemy-api] non-success for {}: {} — {}", url, status, &body[..body.len().min(500)]);
                last_err = Some(anyhow!("API returned {}: {}", status, body));
            }
            Err(e) => {
                last_err = Some(anyhow!("Request failed: {}", e));
            }
        }

        if attempt < max_attempts - 1 {
            let backoff = Duration::from_millis(500 * 2u64.pow(attempt));
            tracing::warn!(
                "[udemy-api] attempt {}/{} failed, retrying in {:?}",
                attempt + 1,
                max_attempts,
                backoff
            );
            tokio::time::sleep(backoff).await;
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("All retry attempts failed")))
}

async fn handle_pagination(
    session: &UdemySession,
    initial_url: &str,
    params: Option<&[(&str, &str)]>,
) -> Result<serde_json::Value> {
    let resp = api_get_with_retry(&session.client, initial_url, params).await?;

    let resp_text = resp.text().await
        .map_err(|e| anyhow!("Failed to read response body: {}", e))?;

    let mut data: serde_json::Value = serde_json::from_str(&resp_text)
        .map_err(|e| {
            let preview = if resp_text.len() > 500 { &resp_text[..500] } else { &resp_text };
            tracing::error!("[udemy-api] JSON parse failed. Body preview: {}", preview);
            anyhow!("Failed to parse JSON: {} — body starts with: {}", e, &resp_text[..resp_text.len().min(200)])
        })?;

    if data.get("results").and_then(|r| r.as_array()).is_none() {
        tracing::warn!("[udemy-api] response has no 'results' array. Keys: {:?}",
            data.as_object().map(|o| o.keys().collect::<Vec<_>>()));
        return Ok(data);
    }

    let mut page = 1u32;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(url) = next_page_url(&data) {
        if !seen.insert(url.clone()) {
            tracing::warn!("[udemy-api] pagination loop detected at page {}, stopping", page);
            break;
        }
        page += 1;
        tracing::info!("[udemy-api] fetching page {}", page);

        tokio::time::sleep(Duration::from_millis(200)).await;

        let resp = api_get_with_retry(&session.client, &url, None)
            .await
            .map_err(|e| anyhow!("page {} of {} failed: {}", page, initial_url, e))?;

        let page_data: serde_json::Value = resp.json().await
            .map_err(|e| anyhow!("Failed to parse page {} JSON: {}", page, e))?;

        merge_page(&mut data, page_data);
    }

    if page > 1 {
        tracing::info!(
            "[udemy-api] {} pages merged, {} results",
            page,
            data.get("results").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0)
        );
    }

    Ok(data)
}

pub(crate) fn next_page_url(data: &serde_json::Value) -> Option<String> {
    data.get("next")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn merge_page(data: &mut serde_json::Value, page_data: serde_json::Value) {
    if let Some(new_results) = page_data.get("results").and_then(|r| r.as_array()) {
        if let Some(existing) = data.get_mut("results").and_then(|r| r.as_array_mut()) {
            existing.extend(new_results.iter().cloned());
        }
    }
    data["next"] = page_data.get("next").cloned().unwrap_or(serde_json::Value::Null);
}

pub fn extract_course_locale(value: &serde_json::Value) -> Option<String> {
    let locale = value.get("locale")?;
    if let Some(s) = locale.as_str() {
        return if s.is_empty() { None } else { Some(s.to_string()) };
    }
    locale
        .get("locale")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn parse_course_from_json(item: &serde_json::Value) -> Option<UdemyCourse> {
    let id = item.get("id")?.as_u64()?;
    let title = item.get("title")?.as_str().unwrap_or("").to_string();
    let published_title = item.get("published_title")?.as_str().unwrap_or("").to_string();
    let url = item.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());

    let image_url = item.get("image_240x135")
        .or_else(|| item.get("image_480x270"))
        .or_else(|| item.get("image_url"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let num_published_lectures = item.get("num_published_lectures")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let locale = extract_course_locale(item);

    Some(UdemyCourse {
        id,
        title,
        published_title,
        url,
        image_url,
        num_published_lectures,
        locale,
    })
}

pub async fn list_my_courses(
    session: &UdemySession,
    portal_name: &str,
) -> Result<Vec<UdemyCourse>> {
    let url = format!(
        "https://{}.udemy.com/api-2.0/users/me/subscribed-courses?fields[course]=id,url,title,published_title,image_240x135,num_published_lectures,locale&ordering=-last_accessed,-access_time&page=1&page_size=100",
        portal_name
    );

    tracing::info!("[udemy-api] fetching subscribed courses from {}", portal_name);

    let data = handle_pagination(session, &url, None).await?;

    let results = data.get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let courses: Vec<UdemyCourse> = results
        .iter()
        .filter_map(parse_course_from_json)
        .collect();

    tracing::info!("[udemy-api] found {} subscribed courses", courses.len());
    Ok(courses)
}

pub async fn list_subscription_courses(
    session: &UdemySession,
    portal_name: &str,
) -> Result<Vec<UdemyCourse>> {
    let url = format!(
        "https://{}.udemy.com/api-2.0/users/me/subscription-course-enrollments?fields[course]=id,url,title,published_title,image_240x135,num_published_lectures,locale&page=1&page_size=100",
        portal_name
    );

    tracing::info!("[udemy-api] fetching subscription course enrollments from {}", portal_name);

    let data = handle_pagination(session, &url, None).await?;

    let results = data.get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let courses: Vec<UdemyCourse> = results
        .iter()
        .filter_map(parse_course_from_json)
        .collect();

    tracing::info!("[udemy-api] found {} subscription courses", courses.len());
    Ok(courses)
}

pub async fn list_all_courses(
    session: &UdemySession,
    portal_name: &str,
) -> Result<Vec<UdemyCourse>> {
    let my_courses_result = list_my_courses(session, portal_name).await;
    let sub_courses_result = list_subscription_courses(session, portal_name).await;

    if let (Err(err), Err(_)) = (&my_courses_result, &sub_courses_result) {
        tracing::error!("[udemy-api] both course fetches failed: {}", err);
        return Err(my_courses_result.unwrap_err());
    }

    let mut my_courses = match my_courses_result {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[udemy-api] subscribed courses failed (continuing with subscription): {}", e);
            Vec::new()
        }
    };

    let sub_courses = match sub_courses_result {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[udemy-api] subscription enrollments failed (continuing with subscribed): {}", e);
            Vec::new()
        }
    };

    let existing_ids: std::collections::HashSet<u64> = my_courses.iter().map(|c| c.id).collect();
    for course in sub_courses {
        if !existing_ids.contains(&course.id) {
            my_courses.push(course);
        }
    }

    tracing::info!("[udemy-api] total unique courses: {}", my_courses.len());
    Ok(my_courses)
}

pub fn asset_type_of(asset: &serde_json::Value) -> String {
    asset.get("asset_type")
        .or_else(|| asset.get("assetType"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase()
}

pub fn video_asset_is_drm_only(a: &serde_json::Value) -> bool {
    let has_stream_urls = a.get("stream_urls").map(|v| !v.is_null()).unwrap_or(false);
    let has_media_sources = a.get("media_sources").map(|v| !v.is_null()).unwrap_or(false);
    if has_stream_urls || !has_media_sources {
        return false;
    }

    let is_drm = a.get("course_is_drmed").and_then(|v| v.as_bool()).unwrap_or(false)
        || a.get("media_license_token").map(|v| !v.is_null()).unwrap_or(false);

    let has_downloadable = a.get("media_sources")
        .and_then(|v| v.as_array())
        .map(|sources| sources.iter().any(|s| {
            let t = s.get("type").and_then(|v| v.as_str()).unwrap_or("");
            t == "video/mp4" || t == "application/x-mpegURL"
        }))
        .unwrap_or(false);

    is_drm && !has_downloadable
}

#[derive(Debug, Clone, Serialize)]
pub struct UdemySectionSummary {
    pub id: u64,
    pub index: u32,
    pub title: String,
    pub lecture_count: u32,
    pub video_count: u32,
    pub drm_video_count: u32,
    pub lecture_ids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UdemyCurriculumSummary {
    pub course_id: u64,
    pub title: String,
    pub total_lectures: u32,
    pub total_video_lectures: u32,
    pub drm_video_lectures: u32,
    pub sections: Vec<UdemySectionSummary>,
}

pub fn summarize_curriculum(curriculum: &UdemyCurriculum) -> UdemyCurriculumSummary {
    let sections = curriculum
        .chapters
        .iter()
        .enumerate()
        .map(|(idx, ch)| {
            let mut lecture_count = 0u32;
            let mut video_count = 0u32;
            let mut drm_video_count = 0u32;
            for lecture in &ch.lectures {
                if lecture.lecture_class != "lecture" {
                    continue;
                }
                lecture_count += 1;
                if let Some(asset) = &lecture.asset {
                    if asset_type_of(asset) == "video" {
                        video_count += 1;
                        if video_asset_is_drm_only(asset) {
                            drm_video_count += 1;
                        }
                    }
                }
            }
            UdemySectionSummary {
                id: ch.id,
                index: (idx + 1) as u32,
                title: ch.title.clone(),
                lecture_count,
                video_count,
                drm_video_count,
                lecture_ids: ch.lectures.iter().map(|l| l.id).collect(),
            }
        })
        .collect();

    UdemyCurriculumSummary {
        course_id: curriculum.course_id,
        title: curriculum.title.clone(),
        total_lectures: curriculum.total_lectures,
        total_video_lectures: curriculum.total_video_lectures,
        drm_video_lectures: curriculum.drm_video_lectures,
        sections,
    }
}

pub fn chapter_selected(
    index: u32,
    chapter_id: u64,
    chapter_filter: &std::collections::HashSet<u32>,
    section_ids: &std::collections::HashSet<u64>,
) -> bool {
    (chapter_filter.is_empty() || chapter_filter.contains(&index))
        && (section_ids.is_empty() || section_ids.contains(&chapter_id))
}

pub fn parse_curriculum(course_id: u64, results: &[serde_json::Value]) -> Result<UdemyCurriculum> {
    let mut chapters: Vec<UdemyChapter> = Vec::new();
    let mut current_chapter: Option<UdemyChapter> = None;
    let mut total_lectures: u32 = 0;
    let mut total_video_lectures: u32 = 0;
    let mut drm_video_lectures: u32 = 0;
    let mut course_title = String::new();

    for item in results {
        let class = item.get("_class").and_then(|c| c.as_str()).unwrap_or("");

        match class {
            "chapter" => {
                if let Some(ch) = current_chapter.take() {
                    chapters.push(ch);
                }

                let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let object_index = item.get("object_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

                if course_title.is_empty() && !title.is_empty() {
                    course_title = title.clone();
                }

                current_chapter = Some(UdemyChapter {
                    id,
                    title,
                    object_index,
                    lectures: Vec::new(),
                });
            }
            "lecture" | "quiz" | "practice" => {
                let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let object_index = item.get("object_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let asset = item.get("asset").cloned();

                if class == "lecture" {
                    total_lectures += 1;

                    if let Some(ref a) = asset {
                        let asset_type = asset_type_of(a);

                        if asset_type == "video" {
                            total_video_lectures += 1;
                            if video_asset_is_drm_only(a) {
                                drm_video_lectures += 1;
                            }
                        }
                    }
                }

                let supplementary_assets = item.get("supplementary_assets")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().cloned().collect());

                let lecture = UdemyLecture {
                    id,
                    title,
                    object_index,
                    lecture_class: class.to_string(),
                    asset,
                    supplementary_assets,
                };

                if let Some(ref mut ch) = current_chapter {
                    ch.lectures.push(lecture);
                } else {
                    let mut implicit_chapter = UdemyChapter {
                        id: 0,
                        title: "Introduction".to_string(),
                        object_index: 0,
                        lectures: Vec::new(),
                    };
                    implicit_chapter.lectures.push(lecture);
                    current_chapter = Some(implicit_chapter);
                }
            }
            _ => {}
        }
    }

    if let Some(ch) = current_chapter.take() {
        chapters.push(ch);
    }

    tracing::info!(
        "[udemy-api] curriculum: {} chapters, {} lectures, {} video, {} drm",
        chapters.len(), total_lectures, total_video_lectures, drm_video_lectures
    );

    Ok(UdemyCurriculum {
        course_id,
        title: course_title,
        chapters,
        total_lectures,
        total_video_lectures,
        drm_video_lectures,
    })
}

pub async fn get_course_locale(
    session: &UdemySession,
    portal_name: &str,
    course_id: u64,
) -> Result<Option<String>> {
    let url = format!(
        "https://{}.udemy.com/api-2.0/courses/{}/",
        portal_name, course_id
    );
    let params: &[(&str, &str)] = &[("fields[course]", "locale")];

    let resp = api_get_with_retry(&session.client, &url, Some(params)).await?;
    let data: serde_json::Value = resp.json().await
        .map_err(|e| anyhow!("Failed to parse course locale response: {}", e))?;

    Ok(extract_course_locale(&data))
}

pub async fn get_fresh_lecture_asset(
    session: &UdemySession,
    portal_name: &str,
    course_id: u64,
    lecture_id: u64,
) -> Result<serde_json::Value> {
    let url = format!(
        "https://{}.udemy.com/api-2.0/users/me/subscribed-courses/{}/lectures/{}/",
        portal_name, course_id, lecture_id
    );
    let params: &[(&str, &str)] = &[
        ("fields[lecture]", "asset"),
        ("fields[asset]", "title,filename,asset_type,status,media_license_token,course_is_drmed,media_sources,stream_urls,download_urls,captions"),
    ];

    let resp = api_get_with_retry(&session.client, &url, Some(params)).await?;
    let data: serde_json::Value = resp.json().await
        .map_err(|e| anyhow!("Failed to parse lecture asset response: {}", e))?;

    data.get("asset")
        .filter(|a| !a.is_null())
        .cloned()
        .ok_or_else(|| anyhow!("Lecture response missing asset"))
}

pub async fn get_course_curriculum(
    session: &UdemySession,
    portal_name: &str,
    course_id: u64,
) -> Result<UdemyCurriculum> {
    let url = format!(
        "https://{}.udemy.com/api-2.0/courses/{}/subscriber-curriculum-items/",
        portal_name, course_id
    );

    let params: &[(&str, &str)] = &[
        ("fields[lecture]", "title,object_index,asset,supplementary_assets"),
        ("fields[quiz]", "title,object_index,type"),
        ("fields[practice]", "title,object_index"),
        ("fields[chapter]", "title,object_index"),
        ("fields[asset]", "title,filename,asset_type,status,is_external,media_license_token,course_is_drmed,media_sources,captions,stream_urls,download_urls,external_url,body"),
        ("page_size", "200"),
    ];

    tracing::info!("[udemy-api] fetching curriculum for course {}", course_id);

    let data = handle_pagination(session, &url, Some(params)).await?;

    let results = data.get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    parse_curriculum(course_id, &results)
}

pub async fn get_course_resources(
    session: &UdemySession,
    portal_name: &str,
    course_id: u64,
) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://{}.udemy.com/api-2.0/courses/{}/resources/",
        portal_name, course_id
    );

    tracing::info!("[udemy-api] fetching resources for course {}", course_id);

    let data = match api_get_with_retry(&session.client, &url, None).await {
        Ok(resp) => {
            let text = resp.text().await
                .map_err(|e| anyhow!("Failed to read resources response: {}", e))?;
            serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|e| anyhow!("Failed to parse resources JSON: {}", e))?
        }
        Err(e) => {
            tracing::warn!("[udemy-api] resources not available for course {}: {}", course_id, e);
            return Ok(Vec::new());
        }
    };

    let results = data.get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    tracing::info!("[udemy-api] found {} resources", results.len());
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn next_page_url_null_and_empty_stop() {
        assert_eq!(next_page_url(&json!({"next": null})), None);
        assert_eq!(next_page_url(&json!({"next": ""})), None);
        assert_eq!(next_page_url(&json!({})), None);
        assert_eq!(
            next_page_url(&json!({"next": "https://www.udemy.com/api-2.0/x?page=2"})).as_deref(),
            Some("https://www.udemy.com/api-2.0/x?page=2")
        );
    }

    #[test]
    fn merge_page_appends_results_and_advances_next() {
        let mut data = json!({"results": [1, 2], "next": "p2"});
        merge_page(&mut data, json!({"results": [3], "next": "p3"}));
        assert_eq!(data["results"], json!([1, 2, 3]));
        assert_eq!(next_page_url(&data).as_deref(), Some("p3"));
        merge_page(&mut data, json!({"results": [4]}));
        assert_eq!(data["results"], json!([1, 2, 3, 4]));
        assert_eq!(next_page_url(&data), None);
    }

    #[test]
    fn drm_only_requires_token_and_no_plain_source() {
        let drm = json!({"media_license_token": "abc", "media_sources": [{"type": "application/dash+xml", "src": "x"}]});
        assert!(video_asset_is_drm_only(&drm));
        let plain = json!({"media_license_token": null, "media_sources": [{"type": "video/mp4", "src": "x"}]});
        assert!(!video_asset_is_drm_only(&plain));
        let drm_with_hls = json!({"course_is_drmed": true, "media_sources": [{"type": "application/x-mpegURL", "src": "x"}]});
        assert!(!video_asset_is_drm_only(&drm_with_hls));
        let no_sources = json!({"course_is_drmed": true});
        assert!(!video_asset_is_drm_only(&no_sources));
    }

    #[test]
    fn parse_curriculum_groups_and_summarizes() {
        let items = vec![
            json!({"_class": "chapter", "id": 10, "title": "Intro", "object_index": 1}),
            json!({"_class": "lecture", "id": 100, "title": "Hello", "object_index": 1, "asset": {"asset_type": "Video", "media_sources": [{"type": "video/mp4", "src": "u"}]}}),
            json!({"_class": "quiz", "id": 101, "title": "Q", "object_index": 2}),
            json!({"_class": "chapter", "id": 20, "title": "Deep", "object_index": 3}),
            json!({"_class": "lecture", "id": 200, "title": "Locked", "object_index": 4, "asset": {"asset_type": "Video", "media_license_token": "t", "media_sources": [{"type": "application/dash+xml", "src": "u"}]}}),
            json!({"_class": "lecture", "id": 201, "title": "Notes", "object_index": 5, "asset": {"asset_type": "Article", "body": "<p>x</p>"}}),
        ];
        let cur = parse_curriculum(1, &items).unwrap();
        assert_eq!(cur.chapters.len(), 2);
        assert_eq!(cur.total_lectures, 3);
        assert_eq!(cur.total_video_lectures, 2);
        assert_eq!(cur.drm_video_lectures, 1);

        let summary = summarize_curriculum(&cur);
        assert_eq!(summary.sections.len(), 2);
        assert_eq!(summary.sections[0].index, 1);
        assert_eq!(summary.sections[0].id, 10);
        assert_eq!(summary.sections[0].lecture_count, 1);
        assert_eq!(summary.sections[0].lecture_ids, vec![100, 101]);
        assert_eq!(summary.sections[1].lecture_count, 2);
        assert_eq!(summary.sections[1].video_count, 1);
        assert_eq!(summary.sections[1].drm_video_count, 1);
    }

    #[test]
    fn lecture_before_any_chapter_gets_implicit_section() {
        let items = vec![
            json!({"_class": "lecture", "id": 5, "title": "Orphan", "object_index": 1, "asset": {"asset_type": "Video"}}),
        ];
        let cur = parse_curriculum(1, &items).unwrap();
        assert_eq!(cur.chapters.len(), 1);
        assert_eq!(cur.chapters[0].id, 0);
        assert_eq!(cur.chapters[0].lectures.len(), 1);
    }

    #[test]
    fn chapter_selected_combines_index_filter_and_section_ids() {
        let none_u32: HashSet<u32> = HashSet::new();
        let none_u64: HashSet<u64> = HashSet::new();
        assert!(chapter_selected(3, 30, &none_u32, &none_u64));

        let by_index: HashSet<u32> = [1, 2].into_iter().collect();
        assert!(chapter_selected(2, 30, &by_index, &none_u64));
        assert!(!chapter_selected(3, 30, &by_index, &none_u64));

        let by_id: HashSet<u64> = [30].into_iter().collect();
        assert!(chapter_selected(3, 30, &none_u32, &by_id));
        assert!(!chapter_selected(3, 31, &none_u32, &by_id));
        assert!(!chapter_selected(3, 30, &by_index, &by_id));
    }

    #[test]
    fn chapter_filter_parses_ranges() {
        let f = parse_chapter_filter("1, 3-5,9");
        assert_eq!(f.len(), 5);
        assert!(f.contains(&4) && f.contains(&9) && !f.contains(&2));
    }
}

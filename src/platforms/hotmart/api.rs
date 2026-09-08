use anyhow::anyhow;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use super::auth::HotmartSession;
use super::course_list;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Course {
    pub id: u64,
    pub name: String,
    pub slug: Option<String>,
    pub seller: String,
    pub subdomain: Option<String>,
    pub is_hotmart_club: bool,
    pub price: Option<f64>,
    pub image_url: Option<String>,
    pub category: Option<String>,
    pub external_platform: bool,
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    pub id: String,
    pub name: String,
    pub pages: Vec<PageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    pub hash: String,
    pub name: String,
    pub page_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub hash: String,
    pub name: String,
    pub content: Option<String>,
    pub has_media: bool,
    pub medias: Vec<LessonMedia>,
    pub attachments: Vec<Attachment>,
    pub complementary_readings: Option<Vec<ReadingLink>>,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LessonMedia {
    pub name: String,
    pub code: String,
    pub url: String,
    pub media_type: String,
    pub size: Option<u64>,
    pub duration: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub file_membership_id: String,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingLink {
    pub title: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub url: String,
    pub file_name: Option<String>,
    pub token: Option<String>,
    pub lambda_url: Option<String>,
    pub is_drm: bool,
}

#[derive(Debug, Clone)]
pub struct SubdomainInfo {
    pub product_id: u64,
    pub subdomain: String,
    pub name: Option<String>,
    pub roles: Vec<String>,
}

pub fn navigation_headers(token: &str, slug: &str, product_id: u64) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", token))
            .map_err(|e| anyhow!("Invalid token for header: {}", e))?,
    );
    headers.insert("Origin", HeaderValue::from_static("https://hotmart.com"));
    headers.insert("Referer", HeaderValue::from_static("https://hotmart.com"));
    headers.insert("Pragma", HeaderValue::from_static("no-cache"));
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    headers.insert(
        "slug",
        HeaderValue::from_str(slug)
            .map_err(|e| anyhow!("Invalid slug for header '{}': {}", slug, e))?,
    );
    headers.insert("x-product-id", HeaderValue::from_str(&product_id.to_string())
        .map_err(|e| anyhow!("Invalid product_id for header: {}", e))?);
    Ok(headers)
}

pub async fn get_subdomains(session: &HotmartSession) -> anyhow::Result<Vec<SubdomainInfo>> {
    let resp = session
        .client
        .post("https://api-sec-vlc.hotmart.com/security/oauth/check_token")
        .form(&[("token", &session.token)])
        .send()
        .await?;

    let status = resp.status();
    let body_text = resp.text().await?;

    if !status.is_success() {
        return Err(anyhow!("check_token retornou status {}: {}", status, &body_text[..500.min(body_text.len())]));
    }

    let body: serde_json::Value = serde_json::from_str(&body_text)?;

    if body.get("resources").and_then(|r| r.as_array()).is_none() {
        tracing::warn!(
            "[hotmart] check_token has no 'resources' array; top-level keys: {:?}",
            course_list::top_level_keys(&body)
        );
        return Err(anyhow!("Field 'resources' not found in check_token"));
    }

    Ok(course_list::parse_check_token_resources(&body))
}

const PURCHASES_V2: &str = "https://api-hub.cb.hotmart.com/club-drive-api/rest/v2/purchase/";
/// Products the user got for free (gifted by the creator, free sign-ups) are
/// listed by a sibling endpoint, not among the purchases (#303, #305).
const PURCHASES_FREE_V1: &str = "https://api-hub.cb.hotmart.com/club-drive-api/rest/v1/purchase/free/";
const MAX_PAGES: u32 = 20;

async fn fetch_purchase_page(
    session: &HotmartSession,
    base: &str,
    page: u32,
    label: &str,
) -> anyhow::Result<(Vec<Course>, usize)> {
    let url = format!("{base}?archived=UNARCHIVED&page={page}");
    let resp = session.client.get(&url).send().await?;
    let status = resp.status();
    let body_text = resp.text().await?;

    if !status.is_success() {
        tracing::error!(
            "[hotmart] {} page {} failed: status={}, body={}",
            label,
            page,
            status,
            &body_text[..body_text.len().min(500)]
        );
        return Err(anyhow!("API returned status {}", status));
    }

    let body: serde_json::Value = serde_json::from_str(&body_text)?;
    let page_size = body
        .get("size")
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
        .unwrap_or(usize::MAX as u64) as usize;

    match course_list::parse_course_list(&body) {
        Ok(parsed) => {
            tracing::info!(
                "[hotmart] {} page {}: {} entries parsed from '{}'",
                label,
                page,
                parsed.courses.len(),
                parsed.container
            );
            Ok((parsed.courses, page_size))
        }
        Err(e) => {
            tracing::warn!(
                "[hotmart] {}: unrecognised response; top-level keys: {:?}; shape: {}",
                label,
                e.top_level_keys,
                e.shape
            );
            Err(anyhow!(e))
        }
    }
}

async fn fetch_all_pages(
    session: &HotmartSession,
    base: &str,
    label: &str,
) -> anyhow::Result<Vec<Course>> {
    let mut all: Vec<Course> = Vec::new();
    for page in 1..=MAX_PAGES {
        let (courses, page_size) = fetch_purchase_page(session, base, page, label).await?;
        let got = courses.len();
        for c in courses {
            if !all.iter().any(|existing| existing.id == c.id) {
                all.push(c);
            }
        }
        if got == 0 || got < page_size {
            break;
        }
    }
    Ok(all)
}

pub async fn list_courses(session: &HotmartSession) -> anyhow::Result<Vec<Course>> {
    tracing::info!(
        "[hotmart] list_courses: token_len={}, cookies={}, email={}",
        session.token.len(),
        session.cookies.len(),
        session.email
    );
    fetch_all_pages(session, PURCHASES_V2, "purchases").await
}

pub async fn list_free_courses(session: &HotmartSession) -> anyhow::Result<Vec<Course>> {
    let mut courses = fetch_all_pages(session, PURCHASES_FREE_V1, "free products").await?;
    for c in &mut courses {
        c.source = Some("free".into());
    }
    Ok(courses)
}

pub use course_list::ProductDetails;

/// `purchase/products/{id}`: price, and the fields the list omits — the club
/// slug and the `membership.registerAddress` that tells us when the content
/// lives on another platform (MemberKit, Kiwify…).
pub async fn get_course_details(
    session: &HotmartSession,
    product_id: u64,
) -> anyhow::Result<ProductDetails> {
    let url = format!(
        "https://api-hub.cb.hotmart.com/club-drive-api/rest/v2/purchase/products/{}",
        product_id
    );

    let resp = session.client.get(&url).send().await?;

    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("Product details not available (status {})", status));
    }

    let body: serde_json::Value = resp.json().await?;
    Ok(course_list::parse_product_details(&body))
}

pub fn merge_subdomains(courses: &mut [Course], subdomains: &[SubdomainInfo]) {
    for course in courses.iter_mut() {
        if let Some(info) = subdomains.iter().find(|s| s.product_id == course.id) {
            course.subdomain = Some(info.subdomain.clone());
            if course.slug.is_none() {
                course.slug = Some(info.subdomain.clone());
            }
        }

        if course.slug.is_none() && course.subdomain.is_none() {
            course.external_platform = true;
        }
    }
}

pub async fn get_modules(
    session: &HotmartSession,
    slug: &str,
    product_id: u64,
) -> anyhow::Result<Vec<Module>> {
    let resp = session
        .client
        .get("https://api-club-course-consumption-gateway-ga.cb.hotmart.com/v1/navigation")
        .headers(navigation_headers(&session.token, slug, product_id)?)
        .send()
        .await?;

    let status = resp.status();
    let body_text = resp.text().await?;

    if !status.is_success() {
        return Err(anyhow!("get_modules retornou status {}: {}", status, &body_text[..500.min(body_text.len())]));
    }

    let body: serde_json::Value = serde_json::from_str(&body_text)?;

    let modules_json = body
        .get("modules")
        .and_then(|m| m.as_array())
        .or_else(|| body.as_array())
        .ok_or_else(|| anyhow!("Unexpected format in modules response"))?;

    let mut modules = Vec::new();
    for m in modules_json {
        let id = m
            .get("id")
            .map(|v| match v {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();
        let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let pages_json = m
            .get("pages")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();

        let pages = pages_json
            .iter()
            .map(|p| PageInfo {
                hash: p.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                name: p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                page_type: p
                    .get("type")
                    .or_else(|| p.get("pageType"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        modules.push(Module { id, name, pages });
    }

    Ok(modules)
}

pub async fn get_lesson(
    session: &HotmartSession,
    slug: &str,
    product_id: u64,
    page_hash: &str,
) -> anyhow::Result<Lesson> {
    let url = format!(
        "https://api-club-course-consumption-gateway-ga.cb.hotmart.com/v2/web/lessons/{}",
        page_hash
    );
    let resp = session
        .client
        .get(&url)
        .headers(navigation_headers(&session.token, slug, product_id)?)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("get_lesson retornou status {}: {}", status, &body_text[..500.min(body_text.len())]));
    }

    let body: serde_json::Value = resp.json().await?;

    if let Some(msg) = body.get("message").and_then(|v| v.as_str()) {
        return Err(anyhow!("Lesson unavailable: {}", msg));
    }

    let hash = body.get("hash").and_then(|v| v.as_str()).unwrap_or(page_hash).to_string();
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let content = body.get("content").and_then(|v| v.as_str()).map(String::from);
    let locked = body.get("locked").and_then(|v| v.as_bool()).unwrap_or(false);

    let has_media = body
        .get("hasMedia")
        .or_else(|| body.get("hasPlayerMedia"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let medias_arr = body
        .get("medias")
        .or_else(|| body.get("mediasSrc"))
        .and_then(|v| v.as_array());

    let medias: Vec<LessonMedia> = medias_arr
        .map(|arr| {
            arr.iter()
                .map(|m| LessonMedia {
                    name: m.get("name")
                        .or_else(|| m.get("mediaName"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    code: m.get("code")
                        .or_else(|| m.get("mediaCode"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    url: m.get("url")
                        .or_else(|| m.get("mediaSrcUrl"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    media_type: m.get("type")
                        .or_else(|| m.get("mediaType"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    size: m.get("size").and_then(|v| v.as_u64()),
                    duration: m.get("duration").and_then(|v| v.as_u64()),
                })
                .collect()
        })
        .unwrap_or_default();

    let attachments: Vec<Attachment> = body
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| Attachment {
                    file_membership_id: a
                        .get("fileMembershipId")
                        .map(|v| match v {
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::String(s) => s.clone(),
                            _ => String::new(),
                        })
                        .unwrap_or_default(),
                    file_name: a.get("fileName").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let complementary_readings = body
        .get("complementaryReadings")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| ReadingLink {
                    title: r.get("articleName")
                        .or_else(|| r.get("title"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    url: r.get("articleUrl")
                        .or_else(|| r.get("url"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                })
                .collect()
        });

    Ok(Lesson {
        hash,
        name,
        content,
        has_media,
        medias,
        attachments,
        complementary_readings,
        locked,
    })
}

pub async fn get_attachment_url(
    session: &HotmartSession,
    id: &str,
) -> anyhow::Result<AttachmentInfo> {
    let url = format!(
        "https://api-club-hot-club-api.cb.hotmart.com/rest/v3/attachment/{}/download",
        id
    );
    let resp = session
        .client
        .get(&url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", "https://hotmart.com")
        .header("Referer", "https://hotmart.com")
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;
    let body_str = body.to_string();
    let is_drm = body_str.contains("drm-protection");

    let download_url = body
        .get("directDownloadUrl")
        .or_else(|| body.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let file_name = body.get("fileName").and_then(|v| v.as_str()).map(String::from);
    let token = body.get("token").and_then(|v| v.as_str()).map(String::from);
    let lambda_url = body.get("lambdaUrl").and_then(|v| v.as_str()).map(String::from);

    Ok(AttachmentInfo {
        url: download_url,
        file_name,
        token,
        lambda_url,
        is_drm,
    })
}

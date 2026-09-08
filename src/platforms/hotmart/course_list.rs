use serde_json::Value;

use super::api::{Course, SubdomainInfo};

const MAX_SEARCH_DEPTH: usize = 6;
const SHAPE_LIMIT: usize = 300;

const KNOWN_CONTAINERS: [&[&str]; 10] = [
    &["data"],
    &["purchases"],
    &["data", "purchases"],
    &["data", "content"],
    &["content"],
    &["items"],
    &["products"],
    &["memberships"],
    &["resources"],
    &["result"],
];

const ID_KEYS: [&str; 4] = ["id", "productId", "product_id", "productID"];
const NAME_KEYS: [&str; 4] = ["name", "productName", "product_name", "title"];
const URL_KEYS: [&str; 8] = [
    "externalUrl",
    "external_url",
    "accessUrl",
    "access_url",
    "membershipUrl",
    "contentUrl",
    "url",
    "link",
];
const IMAGE_KEYS: [&str; 5] = ["picture", "image", "imageUrl", "image_url", "thumbnail"];
const PRODUCER_ROLES: [&str; 8] = [
    "OWNER",
    "PRODUCER",
    "CO_PRODUCER",
    "COPRODUCER",
    "ADMIN",
    "MODERATOR",
    "TEACHER",
    "SELLER",
];

#[derive(Debug, Clone, PartialEq)]
pub struct UnrecognizedFormat {
    pub top_level_keys: Vec<String>,
    pub shape: String,
}

impl std::fmt::Display for UnrecognizedFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hotmart returned the course list in a format OmniGet does not recognise yet. \
             Top-level keys: [{}]. Shape: {}. \
             Please open an issue at https://github.com/tonhowtf/omniget/issues and paste this whole message \
             (it contains only field names, no personal data). \
             The same shape is printed in the OmniGet log (run the app from a terminal to see it).",
            self.top_level_keys.join(", "),
            self.shape
        )
    }
}

impl std::error::Error for UnrecognizedFormat {}

#[derive(Debug)]
pub struct ParsedCourseList {
    pub courses: Vec<Course>,
    pub container: String,
}

pub fn parse_course_list(body: &Value) -> Result<ParsedCourseList, UnrecognizedFormat> {
    if is_empty_page(body) {
        return Ok(ParsedCourseList {
            courses: Vec::new(),
            container: "empty".into(),
        });
    }
    if let Some((entries, container)) = find_known_container(body) {
        return Ok(ParsedCourseList {
            courses: entries.iter().filter_map(parse_entry).collect(),
            container,
        });
    }

    if let Some((entries, path)) = find_course_array(body, "$", 0) {
        return Ok(ParsedCourseList {
            courses: entries.iter().filter_map(parse_entry).collect(),
            container: format!("search:{}", path),
        });
    }

    Err(UnrecognizedFormat {
        top_level_keys: top_level_keys(body),
        shape: describe_shape(body),
    })
}

/// `{"size":1000,"page":1,"allArchivedProducts":false}` is what the purchase
/// endpoints answer when there is nothing to list: the `data` array is simply
/// left out. Only scalars at the top level means "no courses", not "unknown
/// format".
fn is_empty_page(body: &Value) -> bool {
    match body {
        Value::Object(map) => !map.is_empty() && map.values().all(|v| !v.is_array() && !v.is_object()),
        _ => false,
    }
}

/// Price, club slug and external URL from `purchase/products/{id}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProductDetails {
    pub price: Option<f64>,
    pub slug: Option<String>,
    pub external_url: Option<String>,
}

pub fn parse_product_details(body: &Value) -> ProductDetails {
    let product = body.get("product").filter(|p| p.is_object()).unwrap_or(body);
    let price = body
        .get("purchases")
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|purchase| purchase.get("value"))
        .and_then(|v| v.as_f64())
        .or_else(|| body.get("value").and_then(|v| v.as_f64()));
    ProductDetails {
        price,
        slug: extract_slug(body, product),
        external_url: extract_external_url(body, product),
    }
}

pub fn top_level_keys(body: &Value) -> Vec<String> {
    match body {
        Value::Object(map) => map.keys().cloned().collect(),
        Value::Array(_) => vec!["<array>".into()],
        _ => vec![format!("<{}>", type_name(body))],
    }
}

pub fn describe_shape(body: &Value) -> String {
    let mut out = String::new();
    write_shape(body, 0, &mut out);
    if out.len() > SHAPE_LIMIT {
        let mut cut = SHAPE_LIMIT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

pub fn parse_check_token_resources(body: &Value) -> Vec<SubdomainInfo> {
    let Some(resources) = body.get("resources").and_then(|r| r.as_array()) else {
        return Vec::new();
    };

    resources
        .iter()
        .filter_map(|entry| {
            let resource = entry.get("resource").unwrap_or(entry);
            let product_id = extract_id(resource)?;
            let subdomain = resource
                .get("subdomain")
                .or_else(|| resource.get("slug"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())?
                .to_string();
            let name = NAME_KEYS
                .iter()
                .find_map(|k| resource.get(*k).and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty())
                .map(String::from);
            let roles = entry
                .get("roles")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_uppercase())
                        .collect()
                })
                .unwrap_or_default();

            Some(SubdomainInfo {
                product_id,
                subdomain,
                name,
                roles,
            })
        })
        .collect()
}

pub fn is_student_access(info: &SubdomainInfo) -> bool {
    info.roles.is_empty()
        || info
            .roles
            .iter()
            .any(|r| !PRODUCER_ROLES.iter().any(|p| r == p))
}

pub fn append_club_only_courses(courses: &mut Vec<Course>, subdomains: &[SubdomainInfo]) -> usize {
    let mut added = 0;
    for info in subdomains.iter().filter(|s| is_student_access(s)) {
        if courses.iter().any(|c| c.id == info.product_id) {
            continue;
        }
        courses.push(Course {
            id: info.product_id,
            name: info.name.clone().unwrap_or_else(|| info.subdomain.clone()),
            slug: Some(info.subdomain.clone()),
            seller: String::new(),
            subdomain: Some(info.subdomain.clone()),
            is_hotmart_club: true,
            price: None,
            image_url: None,
            category: None,
            external_platform: false,
            external_url: None,
            source: Some("club_access".into()),
        });
        added += 1;
    }
    added
}

fn find_known_container(body: &Value) -> Option<(&Vec<Value>, String)> {
    if let Some(arr) = body.as_array() {
        return Some((arr, "$".into()));
    }
    for path in KNOWN_CONTAINERS {
        let Some(cursor) = path.iter().try_fold(body, |c, k| c.get(*k)) else {
            continue;
        };
        if let Some(arr) = cursor.as_array() {
            if arr.is_empty() || arr.iter().any(looks_like_product) {
                return Some((arr, path.join(".")));
            }
        }
    }
    None
}

fn find_course_array<'a>(value: &'a Value, path: &str, depth: usize) -> Option<(&'a Vec<Value>, String)> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    match value {
        Value::Array(arr) => {
            if !arr.is_empty() && arr.iter().all(|v| v.is_object()) && arr.iter().any(looks_like_product) {
                return Some((arr, path.to_string()));
            }
            arr.iter()
                .enumerate()
                .find_map(|(i, v)| find_course_array(v, &format!("{}[{}]", path, i), depth + 1))
        }
        Value::Object(map) => map
            .iter()
            .find_map(|(k, v)| find_course_array(v, &format!("{}.{}", path, k), depth + 1)),
        _ => None,
    }
}

fn looks_like_product(entry: &Value) -> bool {
    let candidates = [entry, entry.get("product").unwrap_or(entry)];
    candidates.iter().any(|v| {
        v.is_object() && extract_id(v).is_some() && extract_name(v).is_some()
    })
}

fn extract_id(v: &Value) -> Option<u64> {
    ID_KEYS.iter().find_map(|k| {
        let field = v.get(*k)?;
        field
            .as_u64()
            .or_else(|| field.as_str().and_then(|s| s.parse::<u64>().ok()))
            .filter(|id| *id > 0)
    })
}

fn extract_name(v: &Value) -> Option<String> {
    NAME_KEYS
        .iter()
        .find_map(|k| v.get(*k).and_then(|f| f.as_str()))
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}

fn extract_seller(entry: &Value, product: &Value) -> String {
    let containers = [
        product.get("seller"),
        product.get("producer"),
        entry.get("producer"),
        entry.get("seller"),
    ];
    containers
        .iter()
        .flatten()
        .find_map(|s| {
            s.get("name")
                .and_then(|n| n.as_str())
                .or_else(|| s.as_str())
        })
        .unwrap_or("")
        .to_string()
}

/// Club slug from either the old `<slug>.club.hotmart.com` host or the
/// current `hotmart.com/<lang>/club/<slug>/…` link.
fn club_slug_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if let Some(slug) = host.strip_suffix(".club.hotmart.com") {
        return (!slug.is_empty()).then(|| slug.to_string());
    }
    if host == "hotmart.com" || host == "www.hotmart.com" {
        let mut segments = path.split(['/', '?', '#']).filter(|s| !s.is_empty());
        while let Some(seg) = segments.next() {
            if seg == "club" {
                return segments
                    .next()
                    .filter(|s| !s.is_empty() && *s != "public")
                    .map(str::to_string);
            }
        }
    }
    None
}

/// Anything served by Hotmart itself (`club.hotmart.com/oauth/login`,
/// `hotmart.com/…/club/…`) is not an external platform.
fn is_hotmart_url(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|rest| rest.split('/').next())
        .map(|host| host == "hotmart.com" || host.ends_with(".hotmart.com"))
        .unwrap_or(false)
}

fn url_fields<'a>(entry: &'a Value, product: &'a Value) -> impl Iterator<Item = &'a str> {
    let membership = [
        product.pointer("/membership/registerAddress"),
        entry.pointer("/membership/registerAddress"),
        product.pointer("/hotmartClub/link"),
        entry.pointer("/hotmartClub/link"),
    ];
    membership
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter(|s| s.starts_with("http"))
        .chain(URL_KEYS.iter().flat_map(move |k| {
            [product.get(*k), entry.get(*k)]
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .filter(|s| s.starts_with("http"))
        }))
}

fn extract_slug(entry: &Value, product: &Value) -> Option<String> {
    let direct = [
        product.pointer("/hotmartClub/slug"),
        entry.pointer("/hotmartClub/slug"),
        product.pointer("/club/slug"),
        product.pointer("/club/subdomain"),
        product.get("slug"),
        product.get("subdomain"),
        entry.get("subdomain"),
        entry.get("slug"),
    ];
    let from_field = direct
        .iter()
        .flatten()
        .find_map(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    from_field.or_else(|| url_fields(entry, product).find_map(club_slug_from_url))
}

fn extract_external_url(entry: &Value, product: &Value) -> Option<String> {
    url_fields(entry, product)
        .find(|u| !is_hotmart_url(u))
        .map(String::from)
}

fn parse_entry(entry: &Value) -> Option<Course> {
    let product = entry.get("product").filter(|p| p.is_object()).unwrap_or(entry);
    let id = extract_id(product).or_else(|| extract_id(entry)).unwrap_or(0);
    let name = extract_name(product)
        .or_else(|| extract_name(entry))
        .unwrap_or_default();
    if id == 0 && name.is_empty() {
        return None;
    }

    let slug = extract_slug(entry, product);
    let has_club_access = entry
        .pointer("/accessRights/hasClubAccess")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_hotmart_club = slug.is_some() || has_club_access || product.get("hotmartClub").is_some();

    let category = product.get("category").and_then(|v| v.as_str()).map(String::from);
    let image_url = IMAGE_KEYS
        .iter()
        .find_map(|k| product.get(*k).or_else(|| entry.get(*k)).and_then(|v| v.as_str()))
        .map(String::from);

    Some(Course {
        id,
        name,
        slug,
        seller: extract_seller(entry, product),
        subdomain: None,
        is_hotmart_club,
        price: None,
        image_url,
        category,
        external_platform: false,
        external_url: extract_external_url(entry, product),
        source: Some("purchase".into()),
    })
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn write_shape(v: &Value, depth: usize, out: &mut String) {
    if out.len() > SHAPE_LIMIT {
        return;
    }
    match v {
        Value::Object(map) => {
            if depth >= 3 {
                out.push_str("{…}");
                return;
            }
            out.push('{');
            for (i, (k, val)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push(':');
                write_shape(val, depth + 1, out);
            }
            out.push('}');
        }
        Value::Array(arr) => {
            out.push_str(&format!("[{}×", arr.len()));
            match arr.first() {
                Some(first) => write_shape(first, depth + 1, out),
                None => out.push_str("empty"),
            }
            out.push(']');
        }
        other => out.push_str(type_name(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn purchase(id: u64, name: &str) -> Value {
        json!({
            "product": {
                "id": id,
                "name": name,
                "seller": { "name": "Creator" },
                "hotmartClub": { "slug": format!("club-{}", id) },
                "picture": "https://img.example/cover.png",
                "category": "MARKETING"
            },
            "accessRights": { "hasClubAccess": true }
        })
    }

    #[test]
    fn parses_data_container() {
        let body = json!({ "data": [purchase(1, "A"), purchase(2, "B")] });
        let parsed = parse_course_list(&body).unwrap();
        assert_eq!(parsed.container, "data");
        assert_eq!(parsed.courses.len(), 2);
        assert_eq!(parsed.courses[0].slug.as_deref(), Some("club-1"));
        assert_eq!(parsed.courses[0].seller, "Creator");
        assert!(parsed.courses[0].is_hotmart_club);
        assert_eq!(parsed.courses[0].image_url.as_deref(), Some("https://img.example/cover.png"));
        assert_eq!(parsed.courses[0].source.as_deref(), Some("purchase"));
    }

    #[test]
    fn parses_purchases_container_and_root_array() {
        let body = json!({ "purchases": [purchase(3, "C")] });
        assert_eq!(parse_course_list(&body).unwrap().courses[0].id, 3);

        let body = json!([purchase(4, "D")]);
        let parsed = parse_course_list(&body).unwrap();
        assert_eq!(parsed.container, "$");
        assert_eq!(parsed.courses[0].name, "D");
    }

    #[test]
    fn parses_nested_known_containers() {
        let body = json!({ "data": { "purchases": [purchase(5, "E")] } });
        assert_eq!(parse_course_list(&body).unwrap().container, "data.purchases");

        let body = json!({ "data": { "content": [purchase(6, "F")], "page": 0 } });
        assert_eq!(parse_course_list(&body).unwrap().container, "data.content");

        for key in ["content", "items", "products", "memberships", "resources"] {
            let body = json!({ key: [purchase(7, "G")] });
            let parsed = parse_course_list(&body).unwrap();
            assert_eq!(parsed.container, key);
            assert_eq!(parsed.courses.len(), 1);
        }
    }

    #[test]
    fn empty_known_container_is_not_an_error() {
        let body = json!({ "data": [] });
        let parsed = parse_course_list(&body).unwrap();
        assert!(parsed.courses.is_empty());
    }

    #[test]
    fn falls_back_to_recursive_search() {
        let body = json!({
            "status": "OK",
            "payload": {
                "meta": { "page": 1 },
                "subscriptions": [
                    { "productId": "910", "productName": "Gifted course", "type": "GIFT" }
                ]
            }
        });
        let parsed = parse_course_list(&body).unwrap();
        assert_eq!(parsed.container, "search:$.payload.subscriptions");
        assert_eq!(parsed.courses[0].id, 910);
        assert_eq!(parsed.courses[0].name, "Gifted course");
    }

    #[test]
    fn gifted_entry_without_purchase_wrapper() {
        let body = json!({ "data": [{
            "id": 55,
            "name": "Free access",
            "producer": { "name": "Maker" },
            "subdomain": "free-access"
        }] });
        let parsed = parse_course_list(&body).unwrap();
        let c = &parsed.courses[0];
        assert_eq!(c.seller, "Maker");
        assert_eq!(c.slug.as_deref(), Some("free-access"));
        assert!(c.is_hotmart_club);
    }

    #[test]
    fn slug_from_club_url_and_external_url_detection() {
        let body = json!({ "data": [
            { "product": { "id": 1, "name": "Club by url" }, "accessUrl": "https://minha-escola.club.hotmart.com/lesson/abc" },
            { "product": { "id": 2, "name": "Elsewhere" }, "accessUrl": "https://members.othersite.com/course" }
        ] });
        let parsed = parse_course_list(&body).unwrap();
        assert_eq!(parsed.courses[0].slug.as_deref(), Some("minha-escola"));
        assert!(parsed.courses[0].external_url.is_none());
        assert!(parsed.courses[1].slug.is_none());
        assert_eq!(parsed.courses[1].external_url.as_deref(), Some("https://members.othersite.com/course"));
    }

    #[test]
    fn real_purchase_shape_from_consumer_api() {
        // Shape observed on api-hub …/rest/v2/purchase/?archived=UNARCHIVED (Sept 2026).
        let body = json!({
            "data": [
                { "purchase": { "id": 1, "status": "Completo" },
                  "product": { "id": 6289387, "name": "Especialização", "distributionForm": "cadastros",
                               "seller": { "name": "Alberto" }, "membership": { "userActivationForm": 2 } } },
                { "purchase": { "id": 2, "status": "Completo" },
                  "product": { "id": 3941453, "name": "C++ MasterClass", "seller": { "name": "C++" },
                               "hotmartClub": { "link": "https://hotmart.com/pt-br/club/cppmasterclass/products/3941453/", "slug": "cppmasterclass" },
                               "membership": { "registerAddress": "https://club.hotmart.com/oauth/login", "userActivationForm": 4 } } }
            ],
            "size": 1000, "page": 1, "hasDelayedArchivedProduct": false, "allArchivedProducts": false
        });
        let parsed = parse_course_list(&body).unwrap();
        assert_eq!(parsed.container, "data");
        assert_eq!(parsed.courses.len(), 2);
        assert!(parsed.courses[0].slug.is_none());
        assert!(parsed.courses[0].external_url.is_none());
        assert_eq!(parsed.courses[1].slug.as_deref(), Some("cppmasterclass"));
        assert!(parsed.courses[1].external_url.is_none(), "club.hotmart.com is not external");
        assert!(parsed.courses[1].is_hotmart_club);
    }

    #[test]
    fn empty_page_without_data_is_an_empty_list() {
        let body = json!({ "size": 1000, "page": 1, "allArchivedProducts": false });
        let parsed = parse_course_list(&body).unwrap();
        assert!(parsed.courses.is_empty());
        assert_eq!(parsed.container, "empty");
    }

    #[test]
    fn product_details_expose_price_slug_and_memberkit_url() {
        let body = json!({
            "purchases": [{ "id": 1, "value": 240.0 }],
            "product": { "id": 6289387, "name": "X",
                         "membership": { "registerAddress": "https://dev-eficiente.memberkit.com.br/", "userActivationForm": 2 } }
        });
        let d = parse_product_details(&body);
        assert_eq!(d.price, Some(240.0));
        assert!(d.slug.is_none());
        assert_eq!(d.external_url.as_deref(), Some("https://dev-eficiente.memberkit.com.br/"));

        let club = json!({
            "purchases": [{ "value": 0 }],
            "product": { "id": 1, "name": "Y", "hotmartClub": { "link": "https://hotmart.com/pt-br/club/webscraping/products/1/" },
                         "membership": { "registerAddress": "https://club.hotmart.com/oauth/login" } }
        });
        let d = parse_product_details(&club);
        assert_eq!(d.slug.as_deref(), Some("webscraping"));
        assert!(d.external_url.is_none());
    }

    #[test]
    fn club_slug_from_link_forms() {
        assert_eq!(club_slug_from_url("https://escola.club.hotmart.com/lesson/x").as_deref(), Some("escola"));
        assert_eq!(club_slug_from_url("https://hotmart.com/pt-br/club/cppmasterclass/products/3941453/").as_deref(), Some("cppmasterclass"));
        assert_eq!(club_slug_from_url("https://hotmart.com/club/abc").as_deref(), Some("abc"));
        assert!(club_slug_from_url("https://club.hotmart.com/oauth/login").is_none());
        assert!(club_slug_from_url("https://members.othersite.com/course").is_none());
    }

    #[test]
    fn skips_entries_without_id_or_name() {
        let body = json!({ "data": [ { "product": { "id": 0, "name": "" } }, purchase(9, "Real") ] });
        let parsed = parse_course_list(&body).unwrap();
        assert_eq!(parsed.courses.len(), 1);
        assert_eq!(parsed.courses[0].id, 9);
    }

    #[test]
    fn unrecognized_format_reports_keys_only() {
        let body = json!({
            "token": "SECRET-VALUE",
            "profile": { "email": "someone@example.com", "plans": [1, 2] }
        });
        let err = parse_course_list(&body).unwrap_err();
        assert_eq!(err.top_level_keys, vec!["profile", "token"]);
        assert_eq!(err.shape, "{profile:{email:string, plans:[2×number]}, token:string}");
        let msg = err.to_string();
        assert!(!msg.contains("SECRET-VALUE"));
        assert!(!msg.contains("someone@example.com"));
        assert!(msg.contains("github.com/tonhowtf/omniget/issues"));
    }

    #[test]
    fn shape_is_truncated() {
        let mut map = serde_json::Map::new();
        for i in 0..200 {
            map.insert(format!("key_number_{}", i), json!(1));
        }
        let shape = describe_shape(&Value::Object(map));
        assert!(shape.chars().count() <= SHAPE_LIMIT + 1);
        assert!(shape.ends_with('…'));
    }

    #[test]
    fn check_token_resources_and_club_only_merge() {
        let body = json!({
            "user_name": "someone@example.com",
            "resources": [
                { "resource": { "productId": 1, "subdomain": "bought" }, "roles": ["STUDENT"] },
                { "resource": { "productId": 2, "subdomain": "gift", "productName": "Gifted" }, "roles": ["STUDENT"] },
                { "resource": { "productId": 3, "subdomain": "mine" }, "roles": ["OWNER"] },
                { "resource": { "productId": 4 }, "roles": ["STUDENT"] }
            ]
        });
        let subs = parse_check_token_resources(&body);
        assert_eq!(subs.len(), 3);

        let mut courses = vec![Course {
            id: 1, name: "Bought".into(), slug: None, seller: String::new(), subdomain: None,
            is_hotmart_club: false, price: None, image_url: None, category: None,
            external_platform: false, external_url: None, source: Some("purchase".into()),
        }];
        let added = append_club_only_courses(&mut courses, &subs);
        assert_eq!(added, 1);
        assert_eq!(courses.len(), 2);
        assert_eq!(courses[1].id, 2);
        assert_eq!(courses[1].name, "Gifted");
        assert_eq!(courses[1].slug.as_deref(), Some("gift"));
        assert_eq!(courses[1].source.as_deref(), Some("club_access"));
    }

    #[test]
    fn student_access_when_roles_missing() {
        let info = SubdomainInfo { product_id: 1, subdomain: "x".into(), name: None, roles: vec![] };
        assert!(is_student_access(&info));
        let owner = SubdomainInfo { product_id: 1, subdomain: "x".into(), name: None, roles: vec!["PRODUCER".into()] };
        assert!(!is_student_access(&owner));
    }
}

pub mod commands;
pub mod platforms;
pub mod state;

use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use omniget_plugin_sdk::{OmnigetPlugin, PluginHost};
use crate::state::{CoursesCache, UdemyCoursesCache, KiwifyCoursesCache, GumroadCoursesCache, SkoolCoursesCache, TeachableCoursesCache, KajabiCoursesCache, WondriumCoursesCache, ThinkificCoursesCache, RocketseatCoursesCache};
use crate::platforms::hotmart::auth::HotmartSession;
use crate::platforms::hotmart::api::Course;
use crate::platforms::udemy::auth::UdemySession;
use crate::platforms::udemy::api::UdemyCourse;
use crate::platforms::gumroad::api::GumroadProduct;
use crate::platforms::skool::api::SkoolGroup;
use crate::platforms::kiwify::api::KiwifyCourse;
use crate::platforms::teachable::api::TeachableCourse;
use crate::platforms::kajabi::api::KajabiCourse;
use crate::platforms::greatcourses::api::WondriumCourse;
use crate::platforms::thinkific::api::ThinkificCourse;
use crate::platforms::rocketseat::api::RocketseatCourse;

#[derive(serde::Serialize, Clone)]
struct LoginMethod {
    method_type: String,
    command: String,
    extra_fields: Vec<ExtraField>,
}

#[derive(serde::Serialize, Clone)]
struct ExtraField {
    key: String,
    label: String,
    placeholder: String,
    field_type: String,
}

#[derive(serde::Serialize, Clone)]
struct PlatformCommands {
    check_session: String,
    logout: String,
    list: String,
    refresh: String,
    download: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<String>,
}

#[derive(serde::Serialize, Clone)]
struct PlatformFeatures {
    #[serde(skip_serializing_if = "Option::is_none")]
    captcha_event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    has_search: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    download_arg_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    list_returns_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_subtitle_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string_ids: Option<bool>,
}

#[derive(serde::Serialize, Clone)]
struct PlatformUiConfig {
    id: String,
    name: String,
    color: String,
    icon: String,
    login_methods: Vec<LoginMethod>,
    commands: PlatformCommands,
    features: PlatformFeatures,
}

pub struct CoursesPlugin {
    pub host: Option<Arc<dyn PluginHost>>,
    pub runtime: Arc<tokio::runtime::Runtime>,

    pub hotmart_session: Arc<tokio::sync::Mutex<Option<HotmartSession>>>,
    pub active_downloads: Arc<tokio::sync::Mutex<HashMap<u64, CancellationToken>>>,
    pub courses_cache: Arc<tokio::sync::Mutex<Option<CoursesCache>>>,
    pub session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub udemy_session: Arc<tokio::sync::Mutex<Option<UdemySession>>>,
    pub udemy_courses_cache: Arc<tokio::sync::Mutex<Option<UdemyCoursesCache>>>,
    pub udemy_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub udemy_api_webview: Arc<tokio::sync::Mutex<Option<String>>>,
    pub udemy_api_result: Arc<std::sync::Mutex<Option<String>>>,
    pub kiwify_session: Arc<tokio::sync::Mutex<Option<crate::platforms::kiwify::api::KiwifySession>>>,
    pub kiwify_courses_cache: Arc<tokio::sync::Mutex<Option<KiwifyCoursesCache>>>,
    pub kiwify_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub gumroad_session: Arc<tokio::sync::Mutex<Option<crate::platforms::gumroad::api::GumroadSession>>>,
    pub gumroad_courses_cache: Arc<tokio::sync::Mutex<Option<GumroadCoursesCache>>>,
    pub gumroad_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub skool_session: Arc<tokio::sync::Mutex<Option<crate::platforms::skool::api::SkoolSession>>>,
    pub skool_courses_cache: Arc<tokio::sync::Mutex<Option<SkoolCoursesCache>>>,
    pub skool_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub teachable_session: Arc<tokio::sync::Mutex<Option<crate::platforms::teachable::api::TeachableSession>>>,
    pub teachable_courses_cache: Arc<tokio::sync::Mutex<Option<TeachableCoursesCache>>>,
    pub teachable_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub kajabi_session: Arc<tokio::sync::Mutex<Option<crate::platforms::kajabi::api::KajabiSession>>>,
    pub kajabi_courses_cache: Arc<tokio::sync::Mutex<Option<KajabiCoursesCache>>>,
    pub kajabi_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub thinkific_session: Arc<tokio::sync::Mutex<Option<crate::platforms::thinkific::api::ThinkificSession>>>,
    pub thinkific_courses_cache: Arc<tokio::sync::Mutex<Option<ThinkificCoursesCache>>>,
    pub thinkific_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub rocketseat_session: Arc<tokio::sync::Mutex<Option<crate::platforms::rocketseat::api::RocketseatSession>>>,
    pub rocketseat_courses_cache: Arc<tokio::sync::Mutex<Option<RocketseatCoursesCache>>>,
    pub rocketseat_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    pub wondrium_session: Arc<tokio::sync::Mutex<Option<crate::platforms::greatcourses::api::WondriumSession>>>,
    pub wondrium_courses_cache: Arc<tokio::sync::Mutex<Option<WondriumCoursesCache>>>,
    pub wondrium_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
}

impl Clone for CoursesPlugin {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            runtime: self.runtime.clone(),
            hotmart_session: self.hotmart_session.clone(),
            active_downloads: self.active_downloads.clone(),
            courses_cache: self.courses_cache.clone(),
            session_validated_at: self.session_validated_at.clone(),
            udemy_session: self.udemy_session.clone(),
            udemy_courses_cache: self.udemy_courses_cache.clone(),
            udemy_session_validated_at: self.udemy_session_validated_at.clone(),
            udemy_api_webview: self.udemy_api_webview.clone(),
            udemy_api_result: self.udemy_api_result.clone(),
            kiwify_session: self.kiwify_session.clone(),
            kiwify_courses_cache: self.kiwify_courses_cache.clone(),
            kiwify_session_validated_at: self.kiwify_session_validated_at.clone(),
            gumroad_session: self.gumroad_session.clone(),
            gumroad_courses_cache: self.gumroad_courses_cache.clone(),
            gumroad_session_validated_at: self.gumroad_session_validated_at.clone(),
            skool_session: self.skool_session.clone(),
            skool_courses_cache: self.skool_courses_cache.clone(),
            skool_session_validated_at: self.skool_session_validated_at.clone(),
            teachable_session: self.teachable_session.clone(),
            teachable_courses_cache: self.teachable_courses_cache.clone(),
            teachable_session_validated_at: self.teachable_session_validated_at.clone(),
            kajabi_session: self.kajabi_session.clone(),
            kajabi_courses_cache: self.kajabi_courses_cache.clone(),
            kajabi_session_validated_at: self.kajabi_session_validated_at.clone(),
            thinkific_session: self.thinkific_session.clone(),
            thinkific_courses_cache: self.thinkific_courses_cache.clone(),
            thinkific_session_validated_at: self.thinkific_session_validated_at.clone(),
            rocketseat_session: self.rocketseat_session.clone(),
            rocketseat_courses_cache: self.rocketseat_courses_cache.clone(),
            rocketseat_session_validated_at: self.rocketseat_session_validated_at.clone(),
            wondrium_session: self.wondrium_session.clone(),
            wondrium_courses_cache: self.wondrium_courses_cache.clone(),
            wondrium_session_validated_at: self.wondrium_session_validated_at.clone(),
        }
    }
}

impl CoursesPlugin {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime for plugin");
        Self {
            host: None,
            runtime: Arc::new(runtime),

            hotmart_session: Arc::new(tokio::sync::Mutex::new(None)),
            active_downloads: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            udemy_session: Arc::new(tokio::sync::Mutex::new(None)),
            udemy_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            udemy_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            udemy_api_webview: Arc::new(tokio::sync::Mutex::new(None)),
            udemy_api_result: Arc::new(std::sync::Mutex::new(None)),
            kiwify_session: Arc::new(tokio::sync::Mutex::new(None)),
            kiwify_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            kiwify_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            gumroad_session: Arc::new(tokio::sync::Mutex::new(None)),
            gumroad_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            gumroad_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            skool_session: Arc::new(tokio::sync::Mutex::new(None)),
            skool_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            skool_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            teachable_session: Arc::new(tokio::sync::Mutex::new(None)),
            teachable_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            teachable_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            kajabi_session: Arc::new(tokio::sync::Mutex::new(None)),
            kajabi_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            kajabi_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            thinkific_session: Arc::new(tokio::sync::Mutex::new(None)),
            thinkific_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            thinkific_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            rocketseat_session: Arc::new(tokio::sync::Mutex::new(None)),
            rocketseat_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            rocketseat_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
            wondrium_session: Arc::new(tokio::sync::Mutex::new(None)),
            wondrium_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            wondrium_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

fn get_all_platform_configs() -> Vec<PlatformUiConfig> {
    vec![
        PlatformUiConfig {
            id: "hotmart".into(), name: "Hotmart".into(), color: "#F04E23".into(), icon: "hotmart".into(),
            login_methods: vec![
                LoginMethod { method_type: "browser".into(), command: "hotmart_set_cookies".into(), extra_fields: vec![
                    ExtraField { key: "url".into(), label: "Login URL".into(), placeholder: "https://app.hotmart.com/login".into(), field_type: "hidden".into() },
                    ExtraField { key: "cookie_domains".into(), label: "Cookie Domains".into(), placeholder: ".hotmart.com,.sso.hotmart.com,.consumer.hotmart.com,.api-sec-vlc.hotmart.com".into(), field_type: "hidden".into() },
                    ExtraField { key: "success_url".into(), label: "Success URL".into(), placeholder: "app.hotmart.com/dashboard".into(), field_type: "hidden".into() },
                ] },
                LoginMethod { method_type: "email_password".into(), command: "hotmart_login".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "hotmart_check_session".into(), logout: "hotmart_logout".into(),
                list: "hotmart_list_courses".into(), refresh: "hotmart_refresh_courses".into(),
                download: "start_course_download".into(), cancel: Some("cancel_course_download".into()), search: None,
            },
            features: PlatformFeatures {
                captcha_event: Some("hotmart-auth-captcha".into()), has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("price".into()),
                session_display: None, string_ids: None,
            },
        },
        PlatformUiConfig {
            id: "udemy".into(), name: "Udemy".into(), color: "#A435F0".into(), icon: "udemy".into(),
            login_methods: vec![
                LoginMethod { method_type: "browser".into(), command: "udemy_set_cookies".into(), extra_fields: vec![
                    ExtraField { key: "url".into(), label: "Login URL".into(), placeholder: "https://www.udemy.com/join/login-popup/".into(), field_type: "hidden".into() },
                    ExtraField { key: "cookie_domains".into(), label: "Cookie Domains".into(), placeholder: ".udemy.com,www.udemy.com".into(), field_type: "hidden".into() },
                    ExtraField { key: "success_url".into(), label: "Success URL".into(), placeholder: "udemy.com/home".into(), field_type: "hidden".into() },
                ] },
                LoginMethod { method_type: "email_only".into(), command: "udemy_login".into(), extra_fields: vec![] },
                LoginMethod { method_type: "cookies".into(), command: "udemy_login_cookies".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "udemy_check_session".into(), logout: "udemy_logout".into(),
                list: "udemy_list_courses".into(), refresh: "udemy_refresh_courses".into(),
                download: "start_udemy_course_download".into(), cancel: Some("cancel_udemy_course_download".into()), search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("num_published_lectures".into()),
                session_display: None, string_ids: None,
            },
        },
        PlatformUiConfig {
            id: "kiwify".into(), name: "Kiwify".into(), color: "#22C55E".into(), icon: "kiwify".into(),
            login_methods: vec![
                LoginMethod { method_type: "email_password".into(), command: "kiwify_login".into(), extra_fields: vec![] },
                LoginMethod { method_type: "token".into(), command: "kiwify_login_token".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "kiwify_check_session".into(), logout: "kiwify_logout".into(),
                list: "kiwify_list_courses".into(), refresh: "kiwify_refresh_courses".into(),
                download: "start_kiwify_course_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("seller".into()),
                session_display: None, string_ids: None,
            },
        },
        PlatformUiConfig {
            id: "teachable".into(), name: "Teachable".into(), color: "#4B5563".into(), icon: "teachable".into(),
            login_methods: vec![
                LoginMethod { method_type: "token".into(), command: "teachable_login_token".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "teachable_check_session".into(), logout: "teachable_logout".into(),
                list: "teachable_list_courses".into(), refresh: "teachable_refresh_courses".into(),
                download: "start_teachable_course_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("slug".into()),
                session_display: None, string_ids: Some(true),
            },
        },
        PlatformUiConfig {
            id: "kajabi".into(), name: "Kajabi".into(), color: "#2563EB".into(), icon: "kajabi".into(),
            login_methods: vec![
                LoginMethod { method_type: "token".into(), command: "kajabi_login_token".into(), extra_fields: vec![
                    ExtraField { key: "siteId".into(), label: "Site ID".into(), placeholder: "Enter site ID".into(), field_type: "text".into() },
                ] },
            ],
            commands: PlatformCommands {
                check_session: "kajabi_check_session".into(), logout: "kajabi_logout".into(),
                list: "kajabi_list_courses".into(), refresh: "kajabi_refresh_courses".into(),
                download: "start_kajabi_course_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("slug".into()),
                session_display: None, string_ids: Some(true),
            },
        },
        PlatformUiConfig {
            id: "gumroad".into(), name: "Gumroad".into(), color: "#FF90E8".into(), icon: "gumroad".into(),
            login_methods: vec![
                LoginMethod { method_type: "email_password".into(), command: "gumroad_login".into(), extra_fields: vec![] },
                LoginMethod { method_type: "token".into(), command: "gumroad_login_token".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "gumroad_check_session".into(), logout: "gumroad_logout".into(),
                list: "gumroad_list_products".into(), refresh: "gumroad_refresh_products".into(),
                download: "start_gumroad_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: Some("productJson".into()), list_returns_key: None,
                item_subtitle_field: Some("creator_name".into()),
                session_display: None, string_ids: Some(true),
            },
        },
        PlatformUiConfig {
            id: "skool".into(), name: "Skool".into(), color: "#5865F2".into(), icon: "skool".into(),
            login_methods: vec![
                LoginMethod { method_type: "email_password".into(), command: "skool_login".into(), extra_fields: vec![] },
                LoginMethod { method_type: "token".into(), command: "skool_login_token".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "skool_check_session".into(), logout: "skool_logout".into(),
                list: "skool_list_groups".into(), refresh: "skool_refresh_groups".into(),
                download: "start_skool_course_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("num_courses".into()),
                session_display: None, string_ids: Some(true),
            },
        },
        PlatformUiConfig {
            id: "greatcourses".into(), name: "Wondrium".into(), color: "#1E3A5F".into(), icon: "greatcourses".into(),
            login_methods: vec![
                LoginMethod { method_type: "email_password".into(), command: "wondrium_login".into(), extra_fields: vec![] },
                LoginMethod { method_type: "token".into(), command: "wondrium_login_token".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "wondrium_check_session".into(), logout: "wondrium_logout".into(),
                list: "wondrium_list_courses".into(), refresh: "wondrium_refresh_courses".into(),
                download: "start_wondrium_course_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("slug".into()),
                session_display: None, string_ids: Some(true),
            },
        },
        PlatformUiConfig {
            id: "thinkific".into(), name: "Thinkific".into(), color: "#4A90D9".into(), icon: "thinkific".into(),
            login_methods: vec![
                LoginMethod { method_type: "cookies".into(), command: "thinkific_login".into(), extra_fields: vec![
                    ExtraField { key: "siteUrl".into(), label: "Site URL".into(), placeholder: "https://yourschool.thinkific.com".into(), field_type: "url".into() },
                ] },
            ],
            commands: PlatformCommands {
                check_session: "thinkific_check_session".into(), logout: "thinkific_logout".into(),
                list: "thinkific_list_courses".into(), refresh: "thinkific_refresh_courses".into(),
                download: "start_thinkific_course_download".into(), cancel: None, search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("slug".into()),
                session_display: Some("site_url".into()), string_ids: Some(true),
            },
        },
        PlatformUiConfig {
            id: "rocketseat".into(), name: "Rocketseat".into(), color: "#8257E5".into(), icon: "rocketseat".into(),
            login_methods: vec![
                LoginMethod { method_type: "token".into(), command: "rocketseat_login_token".into(), extra_fields: vec![] },
            ],
            commands: PlatformCommands {
                check_session: "rocketseat_check_session".into(), logout: "rocketseat_logout".into(),
                list: "rocketseat_list_courses".into(), refresh: "rocketseat_refresh_courses".into(),
                download: "start_rocketseat_course_download".into(), cancel: None,
                search: Some("rocketseat_search_courses".into()),
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: Some(true),
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("slug".into()),
                session_display: Some("platform_name".into()), string_ids: Some(true),
            },
        },
    ]
}

impl OmnigetPlugin for CoursesPlugin {
    fn id(&self) -> &str { "courses" }
    fn name(&self) -> &str { "Course Downloader" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }

    fn initialize(&mut self, host: Arc<dyn PluginHost>) -> anyhow::Result<()> {
        self.host = Some(host);
        Ok(())
    }

    fn handle_command(
        &self,
        command: String,
        args: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + 'static>> {
        let plugin = self.clone();
        let runtime_handle = self.runtime.handle().clone();
        Box::pin(async move {
            runtime_handle.spawn(async move {
            fn get_arg<T: serde::de::DeserializeOwned>(args: &serde_json::Value, key: &str) -> Result<T, String> {
                serde_json::from_value(
                    args.get(key).cloned().ok_or_else(|| format!("missing '{}'", key))?
                ).map_err(|e| format!("invalid '{}': {}", key, e))
            }

            match command.as_str() {
                "hotmart_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let password: String = get_arg(&args, "password")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::auth::hotmart_login(host, &plugin, email, password).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "hotmart_set_cookies" => {
                    let cookies: String = get_arg(&args, "cookies")?;
                    let r = commands::auth::hotmart_set_cookies(&plugin, cookies).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "hotmart_check_session" => {
                    let r = commands::auth::hotmart_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "hotmart_logout" => {
                    let r = commands::auth::hotmart_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "hotmart_list_courses" => {
                    let r = commands::courses::hotmart_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "hotmart_refresh_courses" => {
                    let r = commands::courses::hotmart_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "hotmart_get_modules" => {
                    let course_id: u64 = get_arg(&args, "courseId")?;
                    let slug: String = get_arg(&args, "slug")?;
                    let r = commands::courses::hotmart_get_modules(&plugin, course_id, slug).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::downloads::start_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "cancel_course_download" => {
                    let course_id: u64 = get_arg(&args, "courseId")?;
                    let r = commands::downloads::cancel_course_download(&plugin, course_id).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "get_active_downloads" => {
                    let r = commands::downloads::get_active_downloads(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::udemy_auth::udemy_login(host, &plugin, email).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_login_cookies" => {
                    let cookie_json: String = get_arg(&args, "cookieJson")?;
                    let r = commands::udemy_auth::udemy_login_cookies(&plugin, cookie_json).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_set_cookies" => {
                    let cookies: String = get_arg(&args, "cookies")?;
                    let r = commands::udemy_auth::udemy_set_cookies(&plugin, cookies).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_check_session" => {
                    let r = commands::udemy_auth::udemy_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_get_portal" => {
                    let r = commands::udemy_auth::udemy_get_portal(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_logout" => {
                    let r = commands::udemy_auth::udemy_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_list_courses" => {
                    let r = commands::udemy_courses::udemy_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_refresh_courses" => {
                    let r = commands::udemy_courses::udemy_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_udemy_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::udemy_downloads::start_udemy_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "cancel_udemy_course_download" => {
                    let course_id: u64 = get_arg(&args, "courseId")?;
                    let r = commands::udemy_downloads::cancel_udemy_course_download(&plugin, course_id).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kiwify_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let password: String = get_arg(&args, "password")?;
                    let r = commands::kiwify::kiwify_login(&plugin, email, password).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kiwify_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let r = commands::kiwify::kiwify_login_token(&plugin, token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kiwify_check_session" => {
                    let r = commands::kiwify::kiwify_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kiwify_logout" => {
                    let r = commands::kiwify::kiwify_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kiwify_list_courses" => {
                    let r = commands::kiwify::kiwify_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kiwify_refresh_courses" => {
                    let r = commands::kiwify::kiwify_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_kiwify_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::kiwify::start_kiwify_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_request_otp" => {
                    let email: String = get_arg(&args, "email")?;
                    let r = commands::teachable::teachable_request_otp(email).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_verify_otp" => {
                    let email: String = get_arg(&args, "email")?;
                    let otp_code: String = get_arg(&args, "otpCode")?;
                    let r = commands::teachable::teachable_verify_otp(&plugin, email, otp_code).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let r = commands::teachable::teachable_login_token(&plugin, token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_check_session" => {
                    let r = commands::teachable::teachable_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_logout" => {
                    let r = commands::teachable::teachable_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_set_school" => {
                    let school_id: String = get_arg(&args, "schoolId")?;
                    let r = commands::teachable::teachable_set_school(&plugin, school_id).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_list_schools" => {
                    let r = commands::teachable::teachable_list_schools(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_list_courses" => {
                    let r = commands::teachable::teachable_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "teachable_refresh_courses" => {
                    let r = commands::teachable::teachable_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_teachable_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::teachable::start_teachable_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_request_login_link" => {
                    let email: String = get_arg(&args, "email")?;
                    let r = commands::kajabi::kajabi_request_login_link(email).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_verify_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let confirmation_code: String = get_arg(&args, "confirmationCode")?;
                    let login_token: String = get_arg(&args, "loginToken")?;
                    let r = commands::kajabi::kajabi_verify_login(&plugin, email, confirmation_code, login_token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let site_id: String = get_arg(&args, "siteId")?;
                    let r = commands::kajabi::kajabi_login_token(&plugin, token, site_id).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_check_session" => {
                    let r = commands::kajabi::kajabi_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_logout" => {
                    let r = commands::kajabi::kajabi_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_list_sites" => {
                    let r = commands::kajabi::kajabi_list_sites(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_set_site" => {
                    let site_id: String = get_arg(&args, "siteId")?;
                    let r = commands::kajabi::kajabi_set_site(&plugin, site_id).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_list_courses" => {
                    let r = commands::kajabi::kajabi_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "kajabi_refresh_courses" => {
                    let r = commands::kajabi::kajabi_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_kajabi_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::kajabi::start_kajabi_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "gumroad_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let password: String = get_arg(&args, "password")?;
                    let r = commands::gumroad::gumroad_login(&plugin, email, password).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "gumroad_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let r = commands::gumroad::gumroad_login_token(&plugin, token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "gumroad_check_session" => {
                    let r = commands::gumroad::gumroad_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "gumroad_logout" => {
                    let r = commands::gumroad::gumroad_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "gumroad_list_products" => {
                    let r = commands::gumroad::gumroad_list_products(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "gumroad_refresh_products" => {
                    let r = commands::gumroad::gumroad_refresh_products(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_gumroad_download" => {
                    let product_json: String = get_arg(&args, "productJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::gumroad::start_gumroad_download(host, &plugin, product_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "skool_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let password: String = get_arg(&args, "password")?;
                    let r = commands::skool::skool_login(&plugin, email, password).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "skool_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let r = commands::skool::skool_login_token(&plugin, token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "skool_check_session" => {
                    let r = commands::skool::skool_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "skool_logout" => {
                    let r = commands::skool::skool_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "skool_list_groups" => {
                    let r = commands::skool::skool_list_groups(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "skool_refresh_groups" => {
                    let r = commands::skool::skool_refresh_groups(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_skool_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::skool::start_skool_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "wondrium_login" => {
                    let email: String = get_arg(&args, "email")?;
                    let password: String = get_arg(&args, "password")?;
                    let r = commands::greatcourses::wondrium_login(&plugin, email, password).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "wondrium_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let r = commands::greatcourses::wondrium_login_token(&plugin, token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "wondrium_check_session" => {
                    let r = commands::greatcourses::wondrium_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "wondrium_logout" => {
                    let r = commands::greatcourses::wondrium_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "wondrium_list_courses" => {
                    let r = commands::greatcourses::wondrium_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "wondrium_refresh_courses" => {
                    let r = commands::greatcourses::wondrium_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_wondrium_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::greatcourses::start_wondrium_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "thinkific_login" => {
                    let cookies: String = get_arg(&args, "cookies")?;
                    let site_url: String = get_arg(&args, "siteUrl")?;
                    let r = commands::thinkific::thinkific_login(&plugin, cookies, site_url).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "thinkific_check_session" => {
                    let r = commands::thinkific::thinkific_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "thinkific_logout" => {
                    let r = commands::thinkific::thinkific_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "thinkific_list_courses" => {
                    let r = commands::thinkific::thinkific_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "thinkific_refresh_courses" => {
                    let r = commands::thinkific::thinkific_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_thinkific_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::thinkific::start_thinkific_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "rocketseat_login_token" => {
                    let token: String = get_arg(&args, "token")?;
                    let r = commands::rocketseat::rocketseat_login_token(&plugin, token).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "rocketseat_check_session" => {
                    let r = commands::rocketseat::rocketseat_check_session(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "rocketseat_logout" => {
                    let r = commands::rocketseat::rocketseat_logout(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "rocketseat_list_courses" => {
                    let r = commands::rocketseat::rocketseat_list_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "rocketseat_search_courses" => {
                    let query: String = get_arg(&args, "query")?;
                    let r = commands::rocketseat::rocketseat_search_courses(&plugin, query).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "rocketseat_refresh_courses" => {
                    let r = commands::rocketseat::rocketseat_refresh_courses(&plugin).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "start_rocketseat_course_download" => {
                    let course_json: String = get_arg(&args, "courseJson")?;
                    let output_dir: String = get_arg(&args, "outputDir")?;
                    let host = plugin.host.clone().ok_or("not initialized")?;
                    let r = commands::rocketseat::start_rocketseat_course_download(host, &plugin, course_json, output_dir).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "get_platforms" => {
                    let configs = get_all_platform_configs();
                    serde_json::to_value(configs).map_err(|e| e.to_string())
                }
                "get_platform_config" => {
                    let platform: String = get_arg(&args, "platform")?;
                    let configs = get_all_platform_configs();
                    let config = configs.into_iter().find(|c| c.id == platform)
                        .ok_or_else(|| format!("Unknown platform: {}", platform))?;
                    serde_json::to_value(config).map_err(|e| e.to_string())
                }
                _ => Err(format!("Unknown command: {}", command)),
            }
            }).await.map_err(|e| format!("task join error: {}", e))?
        })
    }

    fn commands(&self) -> Vec<String> {
        vec![
            "hotmart_login".into(),
            "hotmart_set_cookies".into(),
            "hotmart_check_session".into(),
            "hotmart_logout".into(),
            "hotmart_list_courses".into(),
            "hotmart_refresh_courses".into(),
            "hotmart_get_modules".into(),
            "start_course_download".into(),
            "cancel_course_download".into(),
            "get_active_downloads".into(),
            "udemy_login".into(),
            "udemy_login_cookies".into(),
            "udemy_set_cookies".into(),
            "udemy_check_session".into(),
            "udemy_get_portal".into(),
            "udemy_logout".into(),
            "udemy_list_courses".into(),
            "udemy_refresh_courses".into(),
            "start_udemy_course_download".into(),
            "cancel_udemy_course_download".into(),
            "kiwify_login".into(),
            "kiwify_login_token".into(),
            "kiwify_check_session".into(),
            "kiwify_logout".into(),
            "kiwify_list_courses".into(),
            "kiwify_refresh_courses".into(),
            "start_kiwify_course_download".into(),
            "teachable_request_otp".into(),
            "teachable_verify_otp".into(),
            "teachable_login_token".into(),
            "teachable_check_session".into(),
            "teachable_logout".into(),
            "teachable_set_school".into(),
            "teachable_list_schools".into(),
            "teachable_list_courses".into(),
            "teachable_refresh_courses".into(),
            "start_teachable_course_download".into(),
            "kajabi_request_login_link".into(),
            "kajabi_verify_login".into(),
            "kajabi_login_token".into(),
            "kajabi_check_session".into(),
            "kajabi_logout".into(),
            "kajabi_list_sites".into(),
            "kajabi_set_site".into(),
            "kajabi_list_courses".into(),
            "kajabi_refresh_courses".into(),
            "start_kajabi_course_download".into(),
            "gumroad_login".into(),
            "gumroad_login_token".into(),
            "gumroad_check_session".into(),
            "gumroad_logout".into(),
            "gumroad_list_products".into(),
            "gumroad_refresh_products".into(),
            "start_gumroad_download".into(),
            "skool_login".into(),
            "skool_login_token".into(),
            "skool_check_session".into(),
            "skool_logout".into(),
            "skool_list_groups".into(),
            "skool_refresh_groups".into(),
            "start_skool_course_download".into(),
            "wondrium_login".into(),
            "wondrium_login_token".into(),
            "wondrium_check_session".into(),
            "wondrium_logout".into(),
            "wondrium_list_courses".into(),
            "wondrium_refresh_courses".into(),
            "start_wondrium_course_download".into(),
            "thinkific_login".into(),
            "thinkific_check_session".into(),
            "thinkific_logout".into(),
            "thinkific_list_courses".into(),
            "thinkific_refresh_courses".into(),
            "start_thinkific_course_download".into(),
            "rocketseat_login_token".into(),
            "rocketseat_check_session".into(),
            "rocketseat_logout".into(),
            "rocketseat_list_courses".into(),
            "rocketseat_search_courses".into(),
            "rocketseat_refresh_courses".into(),
            "start_rocketseat_course_download".into(),
            "get_platforms".into(),
            "get_platform_config".into(),
        ]
    }
}

omniget_plugin_sdk::export_plugin!(CoursesPlugin::new());

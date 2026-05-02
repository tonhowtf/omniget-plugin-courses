pub mod commands;
pub mod platforms;
pub mod settings_reader;
pub mod state;

use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use omniget_plugin_sdk::{OmnigetPlugin, PluginHost};
use crate::state::{CoursesCache, UdemyCoursesCache, KiwifyCoursesCache, RocketseatCoursesCache};
use crate::platforms::hotmart::auth::HotmartSession;
use crate::platforms::udemy::auth::UdemySession;

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
    pub rocketseat_session: Arc<tokio::sync::Mutex<Option<crate::platforms::rocketseat::api::RocketseatSession>>>,
    pub rocketseat_courses_cache: Arc<tokio::sync::Mutex<Option<RocketseatCoursesCache>>>,
    pub rocketseat_session_validated_at: Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
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
            rocketseat_session: self.rocketseat_session.clone(),
            rocketseat_courses_cache: self.rocketseat_courses_cache.clone(),
            rocketseat_session_validated_at: self.rocketseat_session_validated_at.clone(),
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
            rocketseat_session: Arc::new(tokio::sync::Mutex::new(None)),
            rocketseat_courses_cache: Arc::new(tokio::sync::Mutex::new(None)),
            rocketseat_session_validated_at: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

fn get_all_platform_configs() -> Vec<PlatformUiConfig> {
    vec![
        PlatformUiConfig {
            id: "hotmart".into(), name: "Hotmart".into(), color: "#F04E23".into(), icon: "hotmart".into(),
            login_methods: vec![
                LoginMethod { method_type: "browser".into(), command: "hotmart_set_cookies".into(), extra_fields: vec![
                    ExtraField { key: "url".into(), label: "Login URL".into(), placeholder: "https://sso.hotmart.com/login?redirect=https%3A%2F%2Fconsumer.hotmart.com".into(), field_type: "hidden".into() },
                    ExtraField { key: "cookie_domains".into(), label: "Cookie Domains".into(), placeholder: ".hotmart.com,.sso.hotmart.com,.consumer.hotmart.com,.api-sec-vlc.hotmart.com".into(), field_type: "hidden".into() },
                ] },
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
                download: "start_kiwify_course_download".into(), cancel: Some("cancel_kiwify_course_download".into()), search: None,
            },
            features: PlatformFeatures {
                captcha_event: None, has_search: None,
                download_arg_name: None, list_returns_key: None,
                item_subtitle_field: Some("seller".into()),
                session_display: None, string_ids: None,
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
        if let Some(proxy) = host.proxy_config() {
            omniget_core::core::http_client::init_proxy(
                omniget_core::models::settings::ProxySettings {
                    enabled: true,
                    proxy_type: proxy.proxy_type,
                    host: proxy.host,
                    port: proxy.port,
                    username: proxy.username.unwrap_or_default(),
                    password: proxy.password.unwrap_or_default(),
                }
            );
        }
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
                    let cookies_val = args.get("cookies").ok_or("missing 'cookies'")?;
                    let cookies_json = serde_json::to_string(cookies_val).map_err(|e| e.to_string())?;
                    let r = commands::auth::hotmart_set_cookies(&plugin, cookies_json).await?;
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
                "udemy_request_otp" => {
                    let email: String = get_arg(&args, "email")?;
                    commands::udemy_auth::udemy_request_otp(email).await?;
                    serde_json::to_value("otp_sent").map_err(|e| e.to_string())
                }
                "udemy_verify_otp" => {
                    let email: String = get_arg(&args, "email")?;
                    let otp_code: String = get_arg(&args, "otpCode")?;
                    let r = commands::udemy_auth::udemy_verify_otp(&plugin, email, otp_code).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_login_cookies" => {
                    let cookie_json: String = get_arg(&args, "cookieJson")?;
                    let r = commands::udemy_auth::udemy_login_cookies(&plugin, cookie_json).await?;
                    serde_json::to_value(r).map_err(|e| e.to_string())
                }
                "udemy_set_cookies" => {
                    let cookies_val = args.get("cookies").ok_or("missing 'cookies'")?;
                    let cookies_json = serde_json::to_string(cookies_val).map_err(|e| e.to_string())?;
                    let r = commands::udemy_auth::udemy_set_cookies(&plugin, cookies_json).await?;
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
                "cancel_kiwify_course_download" => {
                    let course_id: String = get_arg(&args, "courseId")?;
                    let r = commands::kiwify::cancel_kiwify_course_download(&plugin, &course_id).await?;
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
            "udemy_request_otp".into(),
            "udemy_verify_otp".into(),
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
            "cancel_kiwify_course_download".into(),
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

use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
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


pub struct CoursesCache {
    pub courses: Vec<Course>,
    pub fetched_at: std::time::Instant,
}

pub struct UdemyCoursesCache {
    pub courses: Vec<UdemyCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct KiwifyCoursesCache {
    pub courses: Vec<KiwifyCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct GumroadCoursesCache {
    pub products: Vec<GumroadProduct>,
    pub fetched_at: std::time::Instant,
}

pub struct SkoolCoursesCache {
    pub groups: Vec<SkoolGroup>,
    pub fetched_at: std::time::Instant,
}

pub struct TeachableCoursesCache {
    pub courses: Vec<TeachableCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct KajabiCoursesCache {
    pub courses: Vec<KajabiCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct WondriumCoursesCache {
    pub courses: Vec<WondriumCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct ThinkificCoursesCache {
    pub courses: Vec<ThinkificCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct RocketseatCoursesCache {
    pub courses: Vec<RocketseatCourse>,
    pub fetched_at: std::time::Instant,
}

pub struct CoursesState {
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

impl Default for CoursesState {
    fn default() -> Self {
        Self {
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

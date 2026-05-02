use crate::platforms::hotmart::api::Course;
use crate::platforms::udemy::api::UdemyCourse;
use crate::platforms::kiwify::api::KiwifyCourse;
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

pub struct RocketseatCoursesCache {
    pub courses: Vec<RocketseatCourse>,
    pub fetched_at: std::time::Instant,
}

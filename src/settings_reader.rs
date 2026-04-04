use omniget_core::models::settings::AppSettings;

pub fn load_app_settings() -> AppSettings {
    let path = match dirs::data_dir() {
        Some(d) => d.join("omniget").join("settings.json"),
        None => {
            tracing::warn!("[settings] could not determine data directory, using defaults");
            return AppSettings::default();
        }
    };

    match std::fs::read_to_string(&path) {
        Ok(json) => match serde_json::from_str::<AppSettings>(&json) {
            Ok(settings) => {
                tracing::info!("[settings] loaded from {}", path.display());
                settings
            }
            Err(e) => {
                tracing::warn!("[settings] failed to parse {}: {}, using defaults", path.display(), e);
                AppSettings::default()
            }
        },
        Err(_) => {
            tracing::info!("[settings] no settings file at {}, using defaults", path.display());
            AppSettings::default()
        }
    }
}

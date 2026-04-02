use std::sync::Arc;
use omniget_plugin_sdk::PluginHost;

pub fn emit<T: serde::Serialize>(host: &Arc<dyn PluginHost>, name: &str, payload: &T) {
    let _ = host.emit_event(name, serde_json::to_value(payload).unwrap_or_default());
}

pub fn emit_dyn(host: &dyn PluginHost, name: &str, payload: &impl serde::Serialize) {
    let _ = host.emit_event(name, serde_json::to_value(payload).unwrap_or_default());
}

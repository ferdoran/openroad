use rfd::{MessageButtons, MessageDialog, MessageLevel};
use std::process::Command;
use std::{env, format};
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };

    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    let _ = cmd.spawn();
}

pub fn get_env_url(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

pub fn show_error(message: &str) {
    MessageDialog::new()
        .set_title("OpenRoad Launcher")
        .set_description(message)
        .set_level(MessageLevel::Error)
        .set_buttons(MessageButtons::Ok)
        .show();
}

pub fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

pub fn normalize_base_url(value: &str) -> String {
    if value.ends_with('/') {
        value.to_string()
    } else {
        format!("{value}/")
    }
}

pub fn read_text_resource(path: &str) -> Result<String, String> {
    if !is_http_url(path) {
        return Err(format!("news fetch failed ({path}): URL must be http(s)"));
    }
    ureq::get(path)
        .call()
        .map_err(|e| format!("news fetch failed ({path}): {e}"))?
        .into_string()
        .map_err(|e| format!("news read failed ({path}): {e}"))
}

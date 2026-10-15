//! The `open_window` command: an app route in a window of its own (a popped-out room).
//!
//! Each route gets one window, labelled after the route ([`label`]): opening it again focuses
//! the open window, and `tauri-plugin-window-state` remembers its size and position across
//! restarts like the main window's.

use tauri::utils::config::WindowConfig;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Prefix of the labels of route windows; the capabilities grant `route-*` the same as `main`.
const LABEL_PREFIX: &str = "route-";
/// Initial inner size of a new route window, in logical pixels.
const SIZE: (f64, f64) = (640.0, 640.0);
/// Smallest inner size of a route window, in logical pixels.
const MIN_SIZE: (f64, f64) = (320.0, 320.0);

/// Opens the app at `route` (a URL hash such as `#/screeps.com/shard0/W8N3/window`) in its
/// own window titled `title`, or focuses the window already showing it.
///
/// Async: creating a window from a synchronous command deadlocks on Windows.
///
/// # Errors
///
/// Returns a message when `route` is not an app route or the window cannot be created.
#[tauri::command]
pub async fn open_window(app: AppHandle, route: String, title: String) -> Result<(), String> {
    let label = label(&route).ok_or_else(|| format!("`{route}` is not an app route"))?;
    if let Some(window) = app.get_webview_window(&label) {
        window.unminimize().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }
    let mut builder = WebviewWindowBuilder::new(
        &app,
        label,
        WebviewUrl::App(format!("index.html{route}").into()),
    )
    .title(title)
    .inner_size(SIZE.0, SIZE.1)
    .min_inner_size(MIN_SIZE.0, MIN_SIZE.1);
    if let Some(arguments) = main_browser_args(&app.config().app.windows) {
        builder = builder.additional_browser_args(arguments);
    }
    builder.build().map(drop).map_err(|error| error.to_string())
}

/// The main window's `WebView2` arguments: every webview of the app shares one `WebView2`
/// environment, which needs the same arguments.
fn main_browser_args(windows: &[WindowConfig]) -> Option<&str> {
    windows
        .iter()
        .find(|window| window.label == "main")
        .and_then(|window| window.additional_browser_args.as_deref())
}

/// The window label of `route`: [`LABEL_PREFIX`] and the route's path with every character
/// outside `[A-Za-z0-9_-]` replaced by `_`; `None` unless `route` is a non-empty `#/…` hash of
/// printable ASCII.
fn label(route: &str) -> Option<String> {
    let path = route.strip_prefix("#/")?;
    if path.is_empty() || !path.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    let mut label = String::with_capacity(LABEL_PREFIX.len() + path.len());
    label.push_str(LABEL_PREFIX);
    label.extend(path.chars().map(|character| {
        if character.is_ascii_alphanumeric() || character == '-' {
            character
        } else {
            '_'
        }
    }));
    Some(label)
}

#[cfg(test)]
mod tests {
    use tauri::utils::config::WindowConfig;

    use super::{label, main_browser_args};

    #[test]
    fn labels_follow_the_route() {
        assert_eq!(
            label("#/localhost-21025/-/W8N3/window").as_deref(),
            Some("route-localhost-21025_-_W8N3_window")
        );
        assert_eq!(
            label("#/screeps.com/shard0/E1S1/window").as_deref(),
            Some("route-screeps_com_shard0_E1S1_window")
        );
        for invalid in ["", "#/", "/a", "#/a b", "#/é"] {
            assert_eq!(label(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn route_windows_take_the_main_windows_browser_arguments() {
        let window = |label: &str, args: Option<&str>| WindowConfig {
            label: label.to_owned(),
            additional_browser_args: args.map(str::to_owned),
            ..WindowConfig::default()
        };
        assert_eq!(main_browser_args(&[]), None);
        assert_eq!(main_browser_args(&[window("main", None)]), None);
        assert_eq!(
            main_browser_args(&[window("other", Some("--x")), window("main", Some("--y"))]),
            Some("--y")
        );
    }
}

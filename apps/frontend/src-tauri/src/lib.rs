use std::sync::OnceLock;

use tauri::{Manager, Url, WebviewWindowBuilder};

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// The app's own base URL, taken from the first page it loads.
///
/// Read back by the navigation handler to build somewhere to send the webview. Recorded
/// rather than derived because the answer is platform- and profile-dependent — the dev
/// server's address under `tauri android dev`, `http://tauri.localhost` from the packaged
/// bundle on Android, `tauri://localhost` on desktop — and Tauri's own `tauri_protocol_url`
/// is `pub(crate)`.
///
/// Captured on page load rather than from `WebviewWindow::url()` in `setup`, where there is
/// nothing to read yet: on Android that returns an empty string and parsing it panics the
/// setup hook with "relative URL without a base". First write wins, which is the app's own
/// index — every later load, including the Stripe return page, leaves it alone.
static APP_URL: OnceLock<Url> = OnceLock::new();

/// The window's own label, as declared in `tauri.conf.json`.
const MAIN_WINDOW: &str = "main";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Mobile only in practice: tauri.conf.json declares the `ourdriveway` scheme
        // under `plugins.deep-link.mobile` and deliberately registers nothing on desktop,
        // where a redirect payment can just come back to the web origin. Initialising it
        // on every platform is harmless — with no scheme registered, nothing is ever
        // routed here.
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_geolocation::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // The window is built here rather than by Tauri's own bootstrap, which is what
            // `"create": false` in tauri.conf.json switches off. Same config, same builder
            // call Tauri would have made (app.rs: `from_config(..)?.build()?`) — the only
            // reason to take it over is that `on_navigation` exists on the builder and
            // nowhere else.
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == MAIN_WINDOW)
                .ok_or("no `main` window in tauri.conf.json")?
                .clone();

            let handle = app.handle().clone();
            WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_navigation(move |url| catch_deep_link(&handle, url))
                .on_page_load(|_, payload| {
                    let _ = APP_URL.set(payload.url().clone());
                })
                .build()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Turns a `ourdriveway://` navigation into an in-app one, and lets everything else pass.
///
/// Stripe returns a redirect payment to `checkout/return.html`, which opens the app's own
/// scheme. In the system browser the OS routes that back as an `ACTION_VIEW` intent and
/// `tauri-plugin-deep-link` picks it up; **inside the webview nothing does**. The plugin
/// only ever listens on `onNewIntent`, and wry has no code that turns a webview navigation
/// into an intent — `shouldOverrideUrlLoading` is wired to this handler and its entire
/// vocabulary is allow-or-block — so the webview tries to load `ourdriveway://` itself and
/// fails with ERR_UNKNOWN_URL_SCHEME. This is the missing half.
///
/// Keyed on the *scheme* on purpose. A rule like "off-origin navigations leave" cannot work
/// here: on Android wry drops `request.isForMainFrame` before this is called, so an iframe
/// is indistinguishable from a top-level navigation, and Stripe's Element is nothing but
/// iframes — including a 3D Secure challenge on the same host the bank redirect uses. A
/// scheme test cannot catch them, because they are all https.
///
/// Returning `false` cancels the navigation, so the error page never renders.
fn catch_deep_link(handle: &tauri::AppHandle, url: &Url) -> bool {
    if url.scheme() != "ourdriveway" {
        return true;
    }

    let Some(mut target) = APP_URL.get().cloned() else {
        return false;
    };

    // `ourdriveway://checkout?session_id=…` parses with "checkout" as the *host* — a custom
    // scheme has no authority — so the two halves are joined back together, and repeated
    // slashes collapsed in case the link was written `ourdriveway:///checkout`. Mirrors the
    // same repair in main.ts, which handles the copies that arrive as intents.
    let joined = format!("/{}{}", url.host_str().unwrap_or_default(), url.path());
    let path = joined.replace("//", "/");

    target.set_path(path.trim_end_matches('/'));
    target.set_query(url.query());

    if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
        // Goes through the dispatcher rather than the webview directly, so the load is
        // posted instead of run inside the navigation callback Android is still in.
        let _ = window.navigate(target);
    }

    false
}

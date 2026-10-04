use std::sync::OnceLock;

use tauri::{Manager, Url, WebviewWindowBuilder};

/// The website's origin, `https://ourdriveway.com`, read from the App Link declared under
/// `plugins.deep-link.mobile` in tauri.conf.json.
///
/// From there rather than a constant or an env file because that entry is already the
/// statement of which site's URLs are the app's own — the links arriving from outside and
/// the ones `catch_deep_link` catches inside the webview are the same URLs.
fn web_origin(config: &tauri::Config) -> Option<String> {
    let link = config.plugins.0.get("deep-link")?.get("mobile")?.get(0)?;
    let scheme = link.get("scheme")?.get(0)?.as_str()?;
    let host = link.get("host")?.as_str()?;
    Some(format!("{scheme}://{host}"))
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
/// index — every later load, including a bank's page during a payment, leaves it alone.
static APP_URL: OnceLock<Url> = OnceLock::new();

/// The window's own label, as declared in `tauri.conf.json`.
const MAIN_WINDOW: &str = "main";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Mobile only in practice: tauri.conf.json declares the App Links for
        // ourdriveway.com under `plugins.deep-link.mobile` and registers nothing on
        // desktop. Initialising it on every platform is harmless — with nothing
        // registered, nothing is ever routed here.
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

            let mut window = WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_page_load(|_, payload| {
                    let _ = APP_URL.set(payload.url().clone());
                });

            // Not under `tauri dev`: there a payment returns to the app's own origin
            // (`webOrigin` in src/api/http.ts), so nothing navigates to the site. `is_dev` and not a debug check — a `tauri build --debug` bundle talks
            // to the real site and needs this as much as a release one.
            if !tauri::is_dev() {
                let web_origin = web_origin(app.config())
                    .ok_or("no deep-link scheme and host in tauri.conf.json")?;
                let handle = app.handle().clone();
                window = window
                    .on_navigation(move |url| catch_deep_link(&handle, &web_origin, url));
            }

            window.build()?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Turns a navigation to the website into the same route inside the app, and lets
/// everything else pass.
///
/// The app and the site share their URLs: a redirect payment returns to
/// `{web_origin}/checkout?session_id=…` on both (api/paymentApi.ts). In the app that whole
/// redirect — bank, Stripe, the return — runs inside the webview, which would load the
/// website there: another origin, and no session, since the refresh cookie lives in
/// reqwest's jar. App Links don't help with this one: Android only hands them over for
/// intents from outside, and **nothing turns a webview navigation into an intent** — wry
/// wires `shouldOverrideUrlLoading` to this handler, whose whole vocabulary is
/// allow-or-block. So this is the in-webview half; the same URLs arriving from outside (a
/// mail or bank app) come in as App Links and are routed by `installDeepLinks` in main.ts.
///
/// Keyed on the site's exact origin, so everything else still loads: Stripe's frames and a
/// 3D Secure challenge are on Stripe's hosts. That matters because on Android wry drops
/// `request.isForMainFrame` before this is called, so an iframe cannot be told apart from
/// a top-level navigation — an iframe of the site itself would be caught too, and the app
/// has none.
///
/// Returning `false` cancels the navigation, so the website never renders in the app.
fn catch_deep_link(handle: &tauri::AppHandle, web_origin: &str, url: &Url) -> bool {
    let Some(mut target) = APP_URL.get().cloned() else {
        return true;
    };
    // Also passes when the app is served from the site's origin itself, which would
    // otherwise send every one of its own loads round again.
    if url.origin().ascii_serialization() != web_origin || url.origin() == target.origin() {
        return true;
    }

    target.set_path(url.path());
    target.set_query(url.query());

    if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
        // Goes through the dispatcher rather than the webview directly, so the load is
        // posted instead of run inside the navigation callback Android is still in.
        let _ = window.navigate(target);
    }

    false
}

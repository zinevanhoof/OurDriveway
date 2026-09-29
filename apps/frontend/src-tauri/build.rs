use std::{env, fs, path::Path};

fn main() {
    web_origin();
    tauri_build::build()
}

/// The website's origin as `WEB_ORIGIN`, for `catch_deep_link` in src/lib.rs.
///
/// Read from `VITE_API_BASE` in the frontend's own env file, so the URL stays written in
/// one place. The file follows the Cargo profile, which is the pairing `tauri dev` and
/// `tauri build` (and their `android` forms) give Vite too: a debug build runs against
/// `.env.development`, a release build against `.env.production`.
fn web_origin() {
    let file = match env::var("PROFILE").as_deref() {
        Ok("release") => ".env.production",
        _ => ".env.development",
    };
    let path = Path::new(&env::var("CARGO_MANIFEST_DIR").expect("set by cargo"))
        .join("..")
        .join(file);
    println!("cargo:rerun-if-changed={}", path.display());

    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let base = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("VITE_API_BASE="))
        .unwrap_or_else(|| panic!("no VITE_API_BASE in {}", path.display()))
        .trim();
    let (scheme, rest) = base
        .split_once("://")
        .unwrap_or_else(|| panic!("VITE_API_BASE={base} in {} is not a URL", path.display()));
    let host = rest.split('/').next().unwrap_or_default();

    println!("cargo:rustc-env=WEB_ORIGIN={scheme}://{host}");
}

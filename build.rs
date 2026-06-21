use std::hash::{Hash, Hasher};

/// Derive a short fingerprint of the bundled static assets at build time and
/// expose it as the `ASSET_HASH` env var. It's appended to the `app.css` /
/// `app.js` URLs as a `?v=` cache-buster, so a browser fetches the new file
/// whenever its contents change — without that, the long `max-age` on
/// `/static/*` would serve a stale copy for up to a day after a deploy.
fn main() {
    let assets = ["static/app.css", "static/app.js", "static/htmx.min.js"];
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in assets {
        println!("cargo:rerun-if-changed={path}");
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        bytes.hash(&mut hasher);
    }
    println!("cargo:rustc-env=ASSET_HASH={:016x}", hasher.finish());
}

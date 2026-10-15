// The web UI is embedded into the binary at compile time (rust-embed), so it
// must be built first. Fail early with instructions instead of a macro error.
fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    // Which release asset this binary updates itself from.
    println!("cargo:rustc-env=TTM_TARGET={}", std::env::var("TARGET").unwrap_or_default());
    if !std::path::Path::new("web/dist/index.html").exists() {
        panic!(
            "web/dist/index.html not found: build the web UI first with \
             `make web` (or `cd web && npm ci && npm run build`)"
        );
    }
}

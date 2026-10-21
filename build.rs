//! Tells the embed where the built page is: `$HERDR_PLANNER_DIST`, or
//! `${XDG_CACHE_HOME:-~/.cache}/herdr-planner/web/dist` (where
//! `scripts/web-build.sh` puts it, outside the repository), or `web/dist`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=HERDR_PLANNER_DIST");
    println!("cargo:rerun-if-env-changed=XDG_CACHE_HOME");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let candidates = [
        std::env::var_os("HERDR_PLANNER_DIST").filter(|v| !v.is_empty()).map(PathBuf::from),
        Some(
            std::env::var_os("XDG_CACHE_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                .unwrap_or_else(|| PathBuf::from(".cache"))
                .join("herdr-planner/web/dist"),
        ),
        Some(manifest.join("web/dist")),
    ];
    let found = candidates
        .iter()
        .flatten()
        .find(|p| p.join("index.html").is_file());
    match found {
        Some(dir) => {
            println!("cargo:rustc-env=HERDR_PLANNER_DIST={}", dir.display());
            println!("cargo:rerun-if-changed={}", dir.display());
        }
        None => {
            eprintln!(
                "herdr-planner: the page is not built. Run scripts/web-build.sh \
                 (or set HERDR_PLANNER_DIST to a built web/dist)."
            );
            std::process::exit(1);
        }
    }
}

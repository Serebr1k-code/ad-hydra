use std::{
    env, fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

const BUILD_HELP: &str =
    "Build the Vue frontend first: cd frontend && bun install --frozen-lockfile && bun run build";
const MAX_ASSETS: usize = 256;
const MAX_BYTES: u64 = 32 * 1024 * 1024;

fn files(path: &Path, output: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", path.display());
    let metadata = fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("{}: {error}. {BUILD_HELP}", path.display()));
    assert!(
        !metadata.file_type().is_symlink(),
        "Frontend symlinks are unsupported: {}",
        path.display()
    );
    if metadata.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("read frontend directory")
            .map(|entry| entry.expect("frontend directory entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            files(&entry, output);
        }
    } else {
        assert!(
            metadata.is_file(),
            "Frontend asset must be a regular file: {}",
            path.display()
        );
        output.push(path.to_owned());
    }
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .expect("frontend modification time")
}

// Directory mtimes detect source removals and renames as well as file edits.
fn newest_input(path: &Path) -> SystemTime {
    let mut newest = modified(path);
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("read frontend input directory") {
            newest = newest.max(newest_input(&entry.expect("frontend input entry").path()));
        }
    }
    newest
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => panic!("Unsupported frontend asset type: {}", path.display()),
    }
}

fn main() {
    let frontend = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest path"))
        .join("../../frontend");
    let dist = frontend.join("dist");
    assert!(
        dist.join("index.html").is_file(),
        "Missing frontend/dist/index.html. {BUILD_HELP}"
    );
    let mut inputs = Vec::new();
    for name in ["src", "index.html", "package.json", "bun.lock"] {
        files(&frontend.join(name), &mut inputs);
    }
    // Watch configuration additions as well as existing inputs.
    println!("cargo:rerun-if-changed={}", frontend.display());
    for entry in fs::read_dir(&frontend).expect("read frontend root") {
        let path = entry.expect("frontend root entry").path();
        let name = path
            .file_name()
            .expect("frontend filename")
            .to_string_lossy();
        if name.starts_with("vite.config.")
            || name.starts_with("tsconfig")
            || name == ".env"
            || name.starts_with(".env.")
        {
            files(&path, &mut inputs);
        }
    }
    let mut assets = Vec::new();
    files(&dist, &mut assets);
    assert!(
        assets.len() <= MAX_ASSETS,
        "Frontend exceeds {MAX_ASSETS} embedded assets"
    );
    assert!(
        assets
            .iter()
            .map(|path| fs::metadata(path).expect("asset metadata").len())
            .sum::<u64>()
            <= MAX_BYTES,
        "Frontend exceeds 32 MiB embedded asset limit"
    );
    let oldest_asset = assets
        .iter()
        .map(|path| modified(path))
        .min()
        .expect("frontend assets");
    assert!(
        inputs.iter().all(|path| modified(path) <= oldest_asset)
            && newest_input(&frontend.join("src")) <= oldest_asset
            && modified(&frontend) <= oldest_asset,
        "Stale frontend/dist: source or configuration is newer than the bundle. {BUILD_HELP}"
    );
    let mut generated = String::from("const ASSETS: &[(&str, &str, &[u8])] = &[\n");
    for path in &assets {
        let relative = path
            .strip_prefix(&dist)
            .expect("asset relative path")
            .to_str()
            .expect("UTF-8 asset name");
        assert!(
            relative == "index.html" || relative.starts_with("assets/"),
            "Unexpected frontend dist file: {relative}"
        );
        let url = if relative == "index.html" {
            "/".to_owned()
        } else {
            format!("/{relative}")
        };
        let absolute = fs::canonicalize(path).expect("absolute asset path");
        generated.push_str(&format!(
            "({url:?}, {:?}, include_bytes!({:?})),\n",
            mime(path),
            absolute.to_str().expect("UTF-8 asset path")
        ));
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("build output")).join("frontend_assets.rs"),
        generated,
    )
    .expect("write embedded frontend map");
}

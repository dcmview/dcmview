use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    emit_build_identity();
    if debug_api_enabled() {
        println!(
            "cargo:warning=The dcmview debug-api feature enables permissive API CORS for debugging. Never enable it outside dcmview debugging contexts."
        );
    }

    // Emit cargo:rerun-if-changed for every file in frontend/src/ recursively,
    // plus the config and lock files. Cargo's directory-level watch does not
    // recurse into subdirectories, so we walk the tree ourselves.
    emit_src_fingerprints(Path::new("frontend/src"));
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/package-lock.json");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/svelte.config.js");
    println!("cargo:rerun-if-changed=frontend/tsconfig.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    println!("cargo:rerun-if-env-changed=DCMVIEW_NODE_PATH");
    println!("cargo:rerun-if-env-changed=DCMVIEW_NPM_PATH");
    println!("cargo:rerun-if-env-changed=DCMVIEW_SKIP_FRONTEND_BUILD");

    if should_skip_frontend_build() {
        ensure_prebuilt_frontend_exists();
        return;
    }

    let _node_bin = match resolve_tool_path("node", "DCMVIEW_NODE_PATH") {
        Ok(path) => path,
        Err(error) => fatal(&error),
    };
    let npm_bin = match resolve_tool_path("npm", "DCMVIEW_NPM_PATH") {
        Ok(path) => path,
        Err(error) => fatal(&error),
    };

    // Only run `npm ci` when package-lock.json content has changed since the last
    // successful install. Persist a SHA-256 digest in OUT_DIR.
    if needs_npm_install() {
        run_npm(&npm_bin, ["ci"]);
        write_install_stamp();
    }

    run_npm(&npm_bin, ["run", "build"]);
}

fn emit_build_identity() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=DCMVIEW_BUILD_TARGET={target}");
    println!("cargo:rustc-env=DCMVIEW_BUILD_PROFILE={profile}");
}

fn debug_api_enabled() -> bool {
    std::env::var_os("CARGO_FEATURE_DEBUG_API").is_some()
}

fn fatal(message: &str) -> ! {
    println!("cargo:error={message}");
    std::process::exit(1);
}

fn should_skip_frontend_build() -> bool {
    matches!(
        std::env::var("DCMVIEW_SKIP_FRONTEND_BUILD").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn ensure_prebuilt_frontend_exists() {
    let required = Path::new("frontend/dist/index.html");
    if !required.is_file() {
        fatal("DCMVIEW_SKIP_FRONTEND_BUILD=1 requires a prebuilt frontend/dist/index.html");
    }
}

// ---------------------------------------------------------------------------
// Fingerprinting helpers
// ---------------------------------------------------------------------------

/// Walk `dir` recursively and emit `cargo:rerun-if-changed` for every file.
fn emit_src_fingerprints(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            emit_src_fingerprints(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// Returns true when package-lock.json has changed since the last npm ci run.
fn needs_npm_install() -> bool {
    let stamp_path = stamp_file_path();
    let current = lock_fingerprint();
    match std::fs::read_to_string(&stamp_path) {
        Ok(saved) => saved.trim() != current.trim(),
        Err(_) => true, // no stamp yet — first build or OUT_DIR was cleaned
    }
}

/// Persist the current package-lock.json digest so the next build can skip
/// `npm ci` if nothing changed.
fn write_install_stamp() {
    if std::env::var("OUT_DIR").is_ok() {
        let _ = std::fs::write(stamp_file_path(), lock_fingerprint());
    }
}

/// Returns the path to the npm-ci stamp file inside Cargo's OUT_DIR.
fn stamp_file_path() -> PathBuf {
    let out_dir = std::env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(out_dir).join("npm-ci.stamp")
}

/// SHA-256 digest of package-lock.json content.
fn lock_fingerprint() -> String {
    let bytes =
        std::fs::read("frontend/package-lock.json").expect("frontend/package-lock.json must exist");
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

// ---------------------------------------------------------------------------
// npm helpers
// ---------------------------------------------------------------------------

fn resolve_tool_path(default_tool: &str, env_var: &str) -> Result<PathBuf, String> {
    if let Ok(raw) = std::env::var(env_var) {
        let configured = raw.trim();
        if configured.is_empty() {
            return Err(format!("{env_var} is set but empty"));
        }
        let candidate = PathBuf::from(configured);
        if !candidate.is_absolute() {
            return Err(format!("{env_var} must be an absolute path when provided"));
        }
        if !tool_runs(&candidate) {
            return Err(format!(
                "{env_var} points to a tool that could not be executed: {}",
                candidate.display()
            ));
        }
        return Ok(candidate);
    }

    let candidate = find_runnable_tool_on_path(default_tool).ok_or_else(|| {
        format!("Node.js and npm are required to build dcmview; missing {default_tool}")
    })?;
    Ok(candidate)
}

fn find_runnable_tool_on_path(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let tool_path = Path::new(tool);
    if tool_path.components().count() > 1 {
        return candidate_names(tool)
            .into_iter()
            .map(PathBuf::from)
            .find(|candidate| candidate.is_file() && tool_runs(candidate));
    }

    std::env::split_paths(&path)
        .flat_map(|dir| {
            candidate_names(tool)
                .into_iter()
                .map(move |name| dir.join(name))
        })
        .find(|candidate| candidate.is_file() && tool_runs(candidate))
}

fn candidate_names(tool: &str) -> Vec<OsString> {
    let base = OsString::from(tool);
    #[cfg(windows)]
    {
        let tool_path = Path::new(tool);
        if tool_path.extension().is_some() {
            return vec![base];
        }
        let pathext =
            std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
        let mut names = Vec::new();
        names.push(base.clone());
        for ext in std::env::split_paths(&pathext) {
            let ext = ext.as_os_str().to_string_lossy();
            if ext.is_empty() {
                continue;
            }
            names.push(OsString::from(format!("{tool}{ext}")));
        }
        names
    }
    #[cfg(not(windows))]
    {
        vec![base]
    }
}

fn tool_runs(tool: &Path) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_npm<'a>(npm_bin: &Path, args: impl IntoIterator<Item = &'a str>) {
    let status = Command::new(npm_bin)
        .args(args)
        .current_dir("frontend")
        .status()
        .expect("failed to run npm for frontend build");

    if !status.success() {
        panic!("frontend npm command failed with status: {status}");
    }
}

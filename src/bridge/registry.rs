use dcmview::server::now_unix_ms;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const VSCODE_BRIDGE_URL_ENV: &str = "DCMVIEW_VSCODE_BRIDGE_URL";
const VSCODE_BRIDGE_TOKEN_ENV: &str = "DCMVIEW_VSCODE_BRIDGE_TOKEN";
pub(super) const VSCODE_BRIDGE_BYPASS_ENV: &str = "DCMVIEW_VSCODE_BYPASS";
const VSCODE_BRIDGE_REGISTRY_DIR_ENV: &str = "DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR";
const VSCODE_BRIDGE_DEBUG_ENV: &str = "DCMVIEW_VSCODE_BRIDGE_DEBUG";
const BRIDGE_REGISTRY_MAX_AGE_MS: u64 = 3 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BridgeEndpoint {
    pub(crate) url: String,
    pub(crate) token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeRegistryEntry {
    bridge_url: String,
    token: String,
    workspace_roots: Option<Vec<String>>,
    created_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistryMatch {
    AllowAny,
    RequireWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgeEnvironment {
    direct_endpoint: Option<BridgeEndpoint>,
    bypass: bool,
    registry: RegistryEnvironment,
}

impl BridgeEnvironment {
    fn capture() -> Self {
        let direct_endpoint = match (
            env::var(VSCODE_BRIDGE_URL_ENV),
            env::var(VSCODE_BRIDGE_TOKEN_ENV),
        ) {
            (Ok(url), Ok(token)) if !url.is_empty() && !token.is_empty() => {
                Some(BridgeEndpoint { url, token })
            }
            _ => None,
        };

        Self {
            direct_endpoint,
            bypass: env::var(VSCODE_BRIDGE_BYPASS_ENV).as_deref() == Ok("1"),
            registry: RegistryEnvironment::capture(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryEnvironment {
    configured_dir: Option<String>,
    state_home: Option<String>,
    home: Option<String>,
    user_profile: Option<String>,
    runtime_dir: Option<String>,
    user: Option<String>,
    temp_dir: PathBuf,
    debug: bool,
}

impl RegistryEnvironment {
    fn capture() -> Self {
        Self {
            configured_dir: env::var(VSCODE_BRIDGE_REGISTRY_DIR_ENV).ok(),
            state_home: env::var("XDG_STATE_HOME").ok(),
            home: env::var("HOME").ok(),
            user_profile: env::var("USERPROFILE").ok(),
            runtime_dir: env::var("XDG_RUNTIME_DIR").ok(),
            user: env::var("USER").or_else(|_| env::var("USERNAME")).ok(),
            temp_dir: env::temp_dir(),
            debug: env::var(VSCODE_BRIDGE_DEBUG_ENV).as_deref() == Ok("1"),
        }
    }

    fn registry_dirs(&self) -> Vec<PathBuf> {
        if let Some(configured) = self.configured_dir.as_deref() {
            if !configured.is_empty() {
                return vec![PathBuf::from(configured)];
            }
        }

        let mut dirs = vec![vscode_bridge_registry_dir_from_values(
            None,
            self.state_home.as_deref(),
            self.home.as_deref(),
            self.user_profile.as_deref(),
        )];
        dirs.extend(legacy_vscode_bridge_registry_dirs_from_values(
            self.runtime_dir.as_deref(),
            self.user.as_deref(),
            &self.temp_dir,
        ));
        dedupe_paths(dirs)
    }
}

/// Endpoints to route a launch through, best first; empty means run locally.
///
/// One rule for every entry point: a process with the bridge environment (a
/// VS Code terminal) may use any live bridge, preferring its own; any other
/// process routes only when `cwd` is inside a registered workspace folder.
pub(crate) fn discover_vscode_bridge_endpoints(cwd: &Path) -> Vec<BridgeEndpoint> {
    let environment = BridgeEnvironment::capture();
    discover_vscode_bridge_endpoints_with_environment(&environment, cwd, now_unix_ms())
}

fn discover_vscode_bridge_endpoints_with_environment(
    environment: &BridgeEnvironment,
    cwd: &Path,
    now_ms: u64,
) -> Vec<BridgeEndpoint> {
    if environment.bypass {
        log_bridge_debug(
            environment.registry.debug,
            "bridge discovery bypassed by DCMVIEW_VSCODE_BYPASS=1",
        );
        return Vec::new();
    }

    if let Some(endpoint) = environment.direct_endpoint.as_ref() {
        log_bridge_debug(
            environment.registry.debug,
            &format!("accepted env endpoint {}", endpoint.url),
        );
    }
    let registry_match = if environment.direct_endpoint.is_some() {
        RegistryMatch::AllowAny
    } else {
        RegistryMatch::RequireWorkspace
    };
    let registry_endpoints = discover_vscode_bridge_registry_endpoints_in_environment(
        &environment.registry,
        cwd,
        registry_match,
        now_ms,
    );
    let endpoints =
        select_bridge_endpoints(environment.direct_endpoint.as_ref(), registry_endpoints);
    log_bridge_debug(
        environment.registry.debug,
        &format!("discovered {} bridge endpoint(s)", endpoints.len()),
    );
    endpoints
}

fn select_bridge_endpoints(
    direct_endpoint: Option<&BridgeEndpoint>,
    registry_endpoints: Vec<BridgeEndpoint>,
) -> Vec<BridgeEndpoint> {
    let mut endpoints = direct_endpoint.cloned().into_iter().collect::<Vec<_>>();
    for endpoint in registry_endpoints {
        if !endpoints.contains(&endpoint) {
            endpoints.push(endpoint);
        }
    }
    endpoints
}

fn discover_vscode_bridge_registry_endpoints_in_environment(
    environment: &RegistryEnvironment,
    cwd: &Path,
    registry_match: RegistryMatch,
    now_ms: u64,
) -> Vec<BridgeEndpoint> {
    let mut endpoints = Vec::new();
    for registry_dir in environment.registry_dirs() {
        for endpoint in discover_vscode_bridge_registry_endpoints_from_dir(
            cwd,
            registry_match,
            now_ms,
            &registry_dir,
            environment.debug,
        ) {
            if !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
    }
    endpoints
}

fn discover_vscode_bridge_registry_endpoints_from_dir(
    cwd: &Path,
    registry_match: RegistryMatch,
    now_ms: u64,
    registry_dir: &Path,
    debug: bool,
) -> Vec<BridgeEndpoint> {
    log_bridge_debug(
        debug,
        &format!("scanning bridge registry dir {}", registry_dir.display()),
    );
    if !registry_dir_is_trusted(registry_dir) {
        log_bridge_debug(
            debug,
            &format!(
                "registry dir untrusted or missing: {}",
                registry_dir.display()
            ),
        );
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(registry_dir) else {
        log_bridge_debug(
            debug,
            &format!("registry dir unreadable: {}", registry_dir.display()),
        );
        return Vec::new();
    };
    let cwd = normalized_path(cwd);
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
            continue;
        };
        if !is_supported_registry_version(&value) {
            log_bridge_debug(
                debug,
                &format!(
                    "registry entry skipped unsupported version: {}",
                    path.display()
                ),
            );
            continue;
        }
        let Ok(registry) = serde_json::from_value::<BridgeRegistryEntry>(value) else {
            let _ = fs::remove_file(&path);
            log_bridge_debug(
                debug,
                &format!("registry entry removed malformed v1: {}", path.display()),
            );
            continue;
        };
        if registry.bridge_url.is_empty() || registry.token.is_empty() {
            log_bridge_debug(
                debug,
                &format!(
                    "registry entry skipped missing endpoint: {}",
                    path.display()
                ),
            );
            continue;
        }
        let Some(created_at) = registry.created_at_ms else {
            let _ = fs::remove_file(&path);
            log_bridge_debug(
                debug,
                &format!(
                    "registry entry removed missing timestamp: {}",
                    path.display()
                ),
            );
            continue;
        };
        if is_expired_registry_entry(created_at, now_ms) {
            let _ = fs::remove_file(&path);
            log_bridge_debug(
                debug,
                &format!("registry entry removed expired: {}", path.display()),
            );
            continue;
        }
        let match_score =
            workspace_match_score(&cwd, registry.workspace_roots.as_deref().unwrap_or(&[]));
        if registry_match == RegistryMatch::RequireWorkspace && match_score == 0 {
            log_bridge_debug(
                debug,
                &format!(
                    "registry entry skipped workspace mismatch: {}",
                    path.display()
                ),
            );
            continue;
        }
        log_bridge_debug(
            debug,
            &format!("registry entry accepted: {}", path.display()),
        );
        candidates.push((
            match_score,
            created_at,
            BridgeEndpoint {
                url: registry.bridge_url,
                token: registry.token,
            },
        ));
    }
    candidates.sort_by(|left, right| (right.0, right.1).cmp(&(left.0, left.1)));

    let mut endpoints = Vec::new();
    for (_, _, endpoint) in candidates {
        if !endpoints.contains(&endpoint) {
            endpoints.push(endpoint);
        }
    }
    endpoints
}

fn is_supported_registry_version(value: &serde_json::Value) -> bool {
    match value.get("version") {
        None => true,
        Some(serde_json::Value::Number(number)) => number.as_u64() == Some(1),
        Some(_) => false,
    }
}

fn registry_dir_is_trusted(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    registry_metadata_is_trusted(&metadata)
}

#[cfg(unix)]
fn registry_metadata_is_trusted(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    registry_ownership_is_trusted(metadata.uid(), metadata.mode(), current_euid())
}

#[cfg(not(unix))]
fn registry_metadata_is_trusted(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn current_euid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn registry_ownership_is_trusted(uid: u32, mode: u32, euid: u32) -> bool {
    uid == euid && mode & 0o022 == 0
}

fn is_expired_registry_entry(created_at_ms: u64, now_ms: u64) -> bool {
    created_at_ms == 0
        || created_at_ms > now_ms.saturating_add(BRIDGE_REGISTRY_MAX_AGE_MS)
        || now_ms.saturating_sub(created_at_ms) > BRIDGE_REGISTRY_MAX_AGE_MS
}

pub(super) fn remove_vscode_bridge_registry_endpoint(endpoint: &BridgeEndpoint) {
    let environment = RegistryEnvironment::capture();
    remove_vscode_bridge_registry_endpoint_in_environment(endpoint, &environment);
}

fn remove_vscode_bridge_registry_endpoint_in_environment(
    endpoint: &BridgeEndpoint,
    environment: &RegistryEnvironment,
) {
    for registry_dir in environment.registry_dirs() {
        remove_vscode_bridge_registry_endpoint_from_dir(endpoint, &registry_dir);
    }
}

fn remove_vscode_bridge_registry_endpoint_from_dir(endpoint: &BridgeEndpoint, registry_dir: &Path) {
    if !registry_dir_is_trusted(registry_dir) {
        return;
    }
    let Ok(entries) = fs::read_dir(registry_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(registry) = serde_json::from_str::<BridgeRegistryEntry>(&contents) else {
            continue;
        };
        if registry.bridge_url == endpoint.url && registry.token == endpoint.token {
            let _ = fs::remove_file(path);
        }
    }
}

fn vscode_bridge_registry_dir_from_values(
    configured: Option<&str>,
    state_home: Option<&str>,
    home: Option<&str>,
    user_profile: Option<&str>,
) -> PathBuf {
    if let Some(configured) = configured {
        if !configured.is_empty() {
            return PathBuf::from(configured);
        }
    }

    if let Some(state_home) = state_home {
        if registry_env_path_is_absolute(state_home) {
            let state_home = PathBuf::from(state_home);
            return state_home.join("dcmview").join("vscode-bridges");
        }
    }

    if let Some(home) = home {
        if registry_env_path_is_absolute(home) {
            let home = PathBuf::from(home);
            return home
                .join(".local")
                .join("state")
                .join("dcmview")
                .join("vscode-bridges");
        }
    }

    if let Some(user_profile) = user_profile {
        if registry_env_path_is_absolute(user_profile) {
            let user_profile = PathBuf::from(user_profile);
            return user_profile
                .join(".local")
                .join("state")
                .join("dcmview")
                .join("vscode-bridges");
        }
    }

    PathBuf::from(".")
        .join(".local")
        .join("state")
        .join("dcmview")
        .join("vscode-bridges")
}

fn legacy_vscode_bridge_registry_dirs_from_values(
    runtime_dir: Option<&str>,
    user: Option<&str>,
    temp_dir: &Path,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(runtime_dir) = runtime_dir {
        if registry_env_path_is_absolute(runtime_dir) {
            let runtime_dir = PathBuf::from(runtime_dir);
            dirs.push(runtime_dir.join("dcmview").join("vscode-bridges"));
        }
    }

    let user = user.unwrap_or("default");
    dirs.push(temp_dir.join(format!(
        "dcmview-vscode-bridges-{}",
        safe_registry_segment(user)
    )));
    dirs
}

fn registry_env_path_is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.as_bytes().get(1) == Some(&b':')
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if !result.contains(&path) {
            result.push(path);
        }
    }
    result
}

pub(super) fn bridge_debug(message: &str) {
    log_bridge_debug(
        env::var(VSCODE_BRIDGE_DEBUG_ENV).as_deref() == Ok("1"),
        message,
    );
}

fn log_bridge_debug(enabled: bool, message: &str) {
    if enabled {
        eprintln!("dcmview bridge: {message}");
    }
}

fn safe_registry_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn normalized_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn workspace_match_score(cwd: &Path, workspace_roots: &[String]) -> usize {
    workspace_roots
        .iter()
        .filter_map(|root| {
            let root = normalized_path(Path::new(root));
            cwd.strip_prefix(&root).ok().map(|_| root.as_os_str().len())
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_environment(registry_dir: &Path) -> RegistryEnvironment {
        RegistryEnvironment {
            configured_dir: Some(registry_dir.display().to_string()),
            state_home: None,
            home: None,
            user_profile: None,
            runtime_dir: None,
            user: None,
            temp_dir: PathBuf::from("/tmp"),
            debug: false,
        }
    }

    fn bridge_environment(
        registry_dir: &Path,
        direct_endpoint: Option<BridgeEndpoint>,
        bypass: bool,
    ) -> BridgeEnvironment {
        BridgeEnvironment {
            direct_endpoint,
            bypass,
            registry: registry_environment(registry_dir),
        }
    }

    #[test]
    fn bridge_selection_prefers_direct_endpoint_and_deduplicates() {
        let direct = BridgeEndpoint {
            url: "http://127.0.0.1:1111".to_string(),
            token: "shared-token".to_string(),
        };
        let registry_only = BridgeEndpoint {
            url: "http://127.0.0.1:2222".to_string(),
            token: "registry-token".to_string(),
        };

        assert_eq!(
            select_bridge_endpoints(
                Some(&direct),
                vec![registry_only.clone(), direct.clone(), registry_only.clone()],
            ),
            vec![direct, registry_only]
        );
    }

    #[test]
    fn bridge_registry_endpoints_prefer_matching_workspace_then_newest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let cwd = workspace.join("nested");
        let now_ms = now_unix_ms();
        fs::create_dir_all(&cwd).expect("workspace dirs");
        fs::write(temp.path().join("invalid.json"), "{").expect("invalid registry");
        fs::write(
            temp.path().join("old.json"),
            serde_json::json!({
                "version": 1,
                "instanceId": "old",
                "bridgeUrl": "http://127.0.0.1:1111",
                "token": "old-token",
                "workspaceRoots": [temp.path().join("elsewhere")],
                "createdAtMs": now_ms
            })
            .to_string(),
        )
        .expect("old registry");
        fs::write(
            temp.path().join("match.json"),
            serde_json::json!({
                "version": 1,
                "instanceId": "match",
                "bridgeUrl": "http://127.0.0.1:2222",
                "token": "match-token",
                "workspaceRoots": [workspace],
                "createdAtMs": now_ms.saturating_sub(1)
            })
            .to_string(),
        )
        .expect("matching registry");
        let matching = BridgeEndpoint {
            url: "http://127.0.0.1:2222".to_string(),
            token: "match-token".to_string(),
        };
        let other = BridgeEndpoint {
            url: "http://127.0.0.1:1111".to_string(),
            token: "old-token".to_string(),
        };
        let direct = BridgeEndpoint {
            url: "http://127.0.0.1:3333".to_string(),
            token: "env-token".to_string(),
        };
        let outside_vscode = bridge_environment(temp.path(), None, false);
        let vscode_terminal = bridge_environment(temp.path(), Some(direct.clone()), false);

        assert_eq!(
            discover_vscode_bridge_registry_endpoints_in_environment(
                &outside_vscode.registry,
                &cwd,
                RegistryMatch::AllowAny,
                now_ms,
            ),
            vec![matching.clone(), other.clone()],
            "workspace match ranks first, then newest"
        );
        assert_eq!(
            discover_vscode_bridge_endpoints_with_environment(&outside_vscode, &cwd, now_ms),
            vec![matching.clone()],
            "without the bridge environment only a containing workspace routes"
        );
        assert!(
            discover_vscode_bridge_endpoints_with_environment(&outside_vscode, temp.path(), now_ms)
                .is_empty(),
            "outside every workspace and outside VS Code, nothing routes"
        );
        assert_eq!(
            discover_vscode_bridge_endpoints_with_environment(
                &vscode_terminal,
                temp.path(),
                now_ms
            ),
            vec![direct, other, matching],
            "a VS Code terminal may use any live bridge after its own"
        );
    }

    #[test]
    fn bridge_registry_matches_shared_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../docs/contracts/vscode-bridge-registry.json"
        ))
        .expect("registry contract parses");

        assert_eq!(
            BRIDGE_REGISTRY_MAX_AGE_MS,
            contract["ttlMs"].as_u64().unwrap()
        );
        for test_case in contract["registryDirs"].as_array().unwrap() {
            let env = test_case["env"].as_object().unwrap();
            let configured = env
                .get(VSCODE_BRIDGE_REGISTRY_DIR_ENV)
                .and_then(|value| value.as_str());
            let state_home = env.get("XDG_STATE_HOME").and_then(|value| value.as_str());
            let home = env.get("HOME").and_then(|value| value.as_str());
            let user_profile = env.get("USERPROFILE").and_then(|value| value.as_str());
            let actual =
                vscode_bridge_registry_dir_from_values(configured, state_home, home, user_profile);
            let expected = PathBuf::from(test_case["expected"].as_str().unwrap());
            assert_eq!(
                actual,
                expected,
                "registry dir contract case {:?}",
                test_case["name"].as_str()
            );
        }
        for test_case in contract["legacyRegistryDirs"].as_array().unwrap() {
            let env = test_case["env"].as_object().unwrap();
            let runtime_dir = env.get("XDG_RUNTIME_DIR").and_then(|value| value.as_str());
            let user = env
                .get("USER")
                .or_else(|| env.get("USERNAME"))
                .and_then(|value| value.as_str());
            let actual = legacy_vscode_bridge_registry_dirs_from_values(
                runtime_dir,
                user,
                Path::new(test_case["tmpDir"].as_str().unwrap()),
            );
            assert!(
                actual.contains(&PathBuf::from(test_case["expected"].as_str().unwrap())),
                "legacy registry dir contract case {:?}",
                test_case["name"].as_str()
            );
        }
        for test_case in contract["safeSegments"].as_array().unwrap() {
            assert_eq!(
                safe_registry_segment(test_case["input"].as_str().unwrap()),
                test_case["expected"].as_str().unwrap()
            );
        }
        for test_case in contract["expiry"]["cases"].as_array().unwrap() {
            assert_eq!(
                is_expired_registry_entry(
                    test_case["createdAtMs"].as_i64().unwrap() as u64,
                    contract["expiry"]["nowMs"].as_u64().unwrap()
                ),
                test_case["expired"].as_bool().unwrap()
            );
        }

        let temp = tempfile::tempdir().expect("tempdir");
        for item in contract["ordering"]["entries"].as_array().unwrap() {
            fs::write(
                temp.path().join(item["file"].as_str().unwrap()),
                item["entry"].to_string(),
            )
            .expect("registry entry");
        }
        let allow_any = discover_vscode_bridge_registry_endpoints_from_dir(
            Path::new(contract["ordering"]["cwd"].as_str().unwrap()),
            RegistryMatch::AllowAny,
            contract["ordering"]["nowMs"].as_u64().unwrap(),
            temp.path(),
            false,
        );
        let require_workspace = discover_vscode_bridge_registry_endpoints_from_dir(
            Path::new(contract["ordering"]["cwd"].as_str().unwrap()),
            RegistryMatch::RequireWorkspace,
            contract["ordering"]["nowMs"].as_u64().unwrap(),
            temp.path(),
            false,
        );

        assert_eq!(
            endpoint_pairs(&allow_any),
            contract["ordering"]["expectedAllowAny"]
        );
        assert_eq!(
            endpoint_pairs(&require_workspace),
            contract["ordering"]["expectedRequireWorkspace"]
        );
    }

    #[test]
    fn bridge_registry_dir_value_helper_has_deterministic_last_resort() {
        let actual = vscode_bridge_registry_dir_from_values(None, None, None, None);

        assert_eq!(
            actual,
            PathBuf::from(".")
                .join(".local")
                .join("state")
                .join("dcmview")
                .join("vscode-bridges")
        );
    }

    #[test]
    fn bridge_discovery_uses_environment_then_registry_and_honors_bypass() {
        let temp = tempfile::tempdir().expect("tempdir");
        let now_ms = now_unix_ms();
        fs::write(
            temp.path().join("registry.json"),
            serde_json::json!({
                "bridgeUrl": "http://127.0.0.1:1111",
                "token": "registry-token",
                "workspaceRoots": [temp.path()],
                "createdAtMs": now_ms
            })
            .to_string(),
        )
        .expect("registry");
        let environment = bridge_environment(
            temp.path(),
            Some(BridgeEndpoint {
                url: "http://127.0.0.1:2222".to_string(),
                token: "env-token".to_string(),
            }),
            false,
        );

        assert_eq!(
            discover_vscode_bridge_endpoints_with_environment(&environment, temp.path(), now_ms),
            vec![
                BridgeEndpoint {
                    url: "http://127.0.0.1:2222".to_string(),
                    token: "env-token".to_string(),
                },
                BridgeEndpoint {
                    url: "http://127.0.0.1:1111".to_string(),
                    token: "registry-token".to_string(),
                },
            ]
        );

        let bypass_environment = bridge_environment(temp.path(), None, true);
        assert!(discover_vscode_bridge_endpoints_with_environment(
            &bypass_environment,
            temp.path(),
            now_ms
        )
        .is_empty());
    }

    #[test]
    fn expired_bridge_registry_entries_are_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let expired_path = temp.path().join("expired.json");
        fs::write(
            &expired_path,
            serde_json::json!({
                "bridgeUrl": "http://127.0.0.1:1111",
                "token": "registry-token",
                "workspaceRoots": [],
                "createdAtMs": 1
            })
            .to_string(),
        )
        .expect("expired registry");

        let endpoints = discover_vscode_bridge_registry_endpoints_from_dir(
            temp.path(),
            RegistryMatch::AllowAny,
            BRIDGE_REGISTRY_MAX_AGE_MS + 2,
            temp.path(),
            false,
        );

        assert!(endpoints.is_empty());
        assert!(!expired_path.exists());
    }

    #[test]
    fn future_registry_versions_are_skipped_not_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let future_path = temp.path().join("future.json");
        let malformed_v1_path = temp.path().join("malformed-v1.json");
        fs::write(
            &future_path,
            serde_json::json!({
                "version": 2,
                "createdAtMs": "future-format"
            })
            .to_string(),
        )
        .expect("future registry");
        fs::write(
            &malformed_v1_path,
            serde_json::json!({
                "version": 1,
                "bridgeUrl": "http://127.0.0.1:1111",
                "token": "token"
            })
            .to_string(),
        )
        .expect("malformed v1 registry");

        let endpoints = discover_vscode_bridge_registry_endpoints_from_dir(
            temp.path(),
            RegistryMatch::AllowAny,
            now_unix_ms(),
            temp.path(),
            false,
        );

        assert!(endpoints.is_empty());
        assert!(future_path.exists());
        assert!(!malformed_v1_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn untrusted_registry_directory_ownership_is_rejected() {
        assert!(registry_ownership_is_trusted(1000, 0o700, 1000));
        assert!(!registry_ownership_is_trusted(1001, 0o700, 1000));
        assert!(!registry_ownership_is_trusted(1000, 0o722, 1000));
    }

    #[test]
    fn removing_bridge_registry_endpoint_deletes_matching_entries_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let stale_path = temp.path().join("stale.json");
        let live_path = temp.path().join("live.json");
        fs::write(
            &stale_path,
            serde_json::json!({
                "bridgeUrl": "http://127.0.0.1:1111",
                "token": "stale-token",
                "workspaceRoots": [],
                "createdAtMs": now_unix_ms()
            })
            .to_string(),
        )
        .expect("stale registry");
        fs::write(
            &live_path,
            serde_json::json!({
                "bridgeUrl": "http://127.0.0.1:2222",
                "token": "live-token",
                "workspaceRoots": [],
                "createdAtMs": now_unix_ms()
            })
            .to_string(),
        )
        .expect("live registry");
        let environment = registry_environment(temp.path());

        remove_vscode_bridge_registry_endpoint_in_environment(
            &BridgeEndpoint {
                url: "http://127.0.0.1:1111".to_string(),
                token: "stale-token".to_string(),
            },
            &environment,
        );

        assert!(!stale_path.exists());
        assert!(live_path.exists());
    }

    fn endpoint_pairs(endpoints: &[BridgeEndpoint]) -> serde_json::Value {
        serde_json::Value::Array(
            endpoints
                .iter()
                .map(|endpoint| serde_json::json!([endpoint.url, endpoint.token]))
                .collect(),
        )
    }
}

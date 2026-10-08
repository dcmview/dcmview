//! The contract between dcmview and a process that starts it.
//!
//! This crate exists so that a parent process, a later workspace crate or
//! another repository can depend on the contract without depending on the
//! viewer. It therefore depends on `serde` only: no axum, no tokio, no DICOM
//! crates.
//!
//! # What belongs here
//!
//! - Now: the launch and startup contract. [`StartupEvent`] (the
//!   `--startup-json` line), [`launch_url`], [`STARTUP_PROTOCOL`],
//!   [`TOKEN_FRAGMENT_PARAM`] and [`TOKEN_ENV_VAR`].
//! - Later: the `--annotation-config` envelope and the `/spoke/v1` wire
//!   types (`docs/design/seams.md`, section 13).
//!
//! The viewer's own HTTP surface (endpoint table, header names, `/api` wire
//! types) is not part of this crate; it lives in `src/api/contracts.rs` of
//! the `dcmview` package, which re-exports the items below.
//!
//! # Rules
//!
//! - Fields are only added. A released field is never removed, renamed or
//!   given a new meaning, because released wrappers and extensions parse
//!   these shapes.
//! - A change an older peer would misread raises [`STARTUP_PROTOCOL`]; an
//!   optional field an older peer may ignore does not
//!   (`docs/design/seams.md`, section 12).
//! - The pinned shapes are the test in this crate. Extend it when a field is
//!   added.

use serde::Serialize;

/// Fragment parameter that carries the token in a launch URL:
/// `http://127.0.0.1:PORT/#token=<token>`. Fragments are never sent to the
/// server, so the token stays out of request logs, proxies and `Referer`.
pub const TOKEN_FRAGMENT_PARAM: &str = "token";
/// Environment variable that fixes the token instead of generating one. The
/// token is never accepted on the command line, which other local users can
/// read.
pub const TOKEN_ENV_VAR: &str = "DCMVIEW_TOKEN";

/// Version of the launch and startup contract in [`StartupEvent`]. It rises
/// when a consumer of the startup line, a flag an integration passes, or a
/// route an integration calls gains a requirement an older peer cannot meet.
pub const STARTUP_PROTOCOL: u32 = 1;

/// The `--startup-json` line the Python wrapper, the VS Code extension and
/// other parents parse. Fields are only ever added.
///
/// `url` is what a browser opens and carries the token fragment, so a
/// consumer that only knows `url` keeps working. `base_url` and `token` are
/// the same two facts apart, for a consumer that rewrites the origin (a
/// forwarded port, `asExternalUri`) and then appends the fragment itself.
///
/// The struct is non-exhaustive because fields are only ever added: build it
/// with [`StartupEvent::tcp`] or [`StartupEvent::unix_socket`], and read the
/// fields you need by name.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct StartupEvent {
    pub r#type: &'static str,
    /// Launch URL with the token fragment. `null` for a Unix socket, where
    /// the local port is whatever the user forwards.
    pub url: Option<String>,
    /// Origin without a fragment or trailing slash. Absent for a Unix socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The bearer token; `null` under `--no-token`.
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Socket path when listening on a Unix socket. Absent for TCP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    pub protocol: u32,
    /// Version of the file-key rules the process applies
    /// (`docs/design/seams.md`, section 12): which key a file gets and how
    /// it is written. A parent that stored records under keys refuses a
    /// child whose number differs. Absent from a process older than file
    /// keys. The number itself is `KEY_RULES` of the `dcmview-annotation`
    /// crate; this crate only carries it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_rules: Option<u32>,
}

/// Never prints the token or the URL that carries it.
impl std::fmt::Debug for StartupEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartupEvent")
            .field("base_url", &self.base_url)
            .field("socket", &self.socket)
            .field("protocol", &self.protocol)
            .field("key_rules", &self.key_rules)
            .finish_non_exhaustive()
    }
}

impl StartupEvent {
    pub const TYPE: &'static str = "server_started";

    /// A TCP listener. `base_url` is the origin, such as
    /// `http://127.0.0.1:43127`; `host` is the bind host as configured.
    pub fn tcp(base_url: &str, host: &str, port: u16, token: Option<&str>) -> Self {
        Self {
            r#type: Self::TYPE,
            url: Some(launch_url(base_url, token)),
            base_url: Some(base_url.to_string()),
            token: token.map(str::to_string),
            host: Some(host.to_string()),
            port: Some(port),
            socket: None,
            protocol: STARTUP_PROTOCOL,
            key_rules: None,
        }
    }

    /// A Unix socket listener (`--unix-socket`).
    pub fn unix_socket(socket: &str, token: Option<&str>) -> Self {
        Self {
            r#type: Self::TYPE,
            url: None,
            base_url: None,
            token: token.map(str::to_string),
            host: None,
            port: None,
            socket: Some(socket.to_string()),
            protocol: STARTUP_PROTOCOL,
            key_rules: None,
        }
    }

    /// The same event reporting the file-key rules version the process
    /// applies.
    pub fn with_key_rules(mut self, key_rules: u32) -> Self {
        self.key_rules = Some(key_rules);
        self
    }
}

/// The URL a browser opens: `<base_url>/#token=<token>`, or `base_url`
/// unchanged under `--no-token`. Tokens are base64url, so they need no
/// escaping in a fragment.
pub fn launch_url(base_url: &str, token: Option<&str>) -> String {
    match token {
        Some(token) => format!("{base_url}/#{TOKEN_FRAGMENT_PARAM}={token}"),
        None => base_url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::StartupEvent;
    use serde_json::json;

    /// The startup line is parsed by released Python wrappers and VS Code
    /// extensions, and by any parent process: these are the exact shapes.
    #[test]
    fn startup_event_matches_the_integration_contract() {
        let cases = [
            (
                StartupEvent::tcp("http://127.0.0.1:43127", "127.0.0.1", 43127, Some("Xy-_09")),
                json!({
                    "type": "server_started",
                    "url": "http://127.0.0.1:43127/#token=Xy-_09",
                    "base_url": "http://127.0.0.1:43127",
                    "token": "Xy-_09",
                    "host": "127.0.0.1",
                    "port": 43127,
                    "protocol": 1
                }),
            ),
            (
                StartupEvent::tcp("http://[::1]:8010", "::1", 8010, None),
                json!({
                    "type": "server_started",
                    "url": "http://[::1]:8010",
                    "base_url": "http://[::1]:8010",
                    "token": null,
                    "host": "::1",
                    "port": 8010,
                    "protocol": 1
                }),
            ),
            (
                StartupEvent::unix_socket("/run/user/1000/dcmview/scan.sock", Some("Xy-_09")),
                json!({
                    "type": "server_started",
                    "url": null,
                    "token": "Xy-_09",
                    "socket": "/run/user/1000/dcmview/scan.sock",
                    "protocol": 1
                }),
            ),
            // What the viewer prints: both listeners with the key-rules
            // version.
            (
                StartupEvent::tcp("http://127.0.0.1:43127", "127.0.0.1", 43127, Some("Xy-_09"))
                    .with_key_rules(1),
                json!({
                    "type": "server_started",
                    "url": "http://127.0.0.1:43127/#token=Xy-_09",
                    "base_url": "http://127.0.0.1:43127",
                    "token": "Xy-_09",
                    "host": "127.0.0.1",
                    "port": 43127,
                    "protocol": 1,
                    "key_rules": 1
                }),
            ),
            (
                StartupEvent::unix_socket("/run/user/1000/dcmview/scan.sock", None)
                    .with_key_rules(1),
                json!({
                    "type": "server_started",
                    "url": null,
                    "token": null,
                    "socket": "/run/user/1000/dcmview/scan.sock",
                    "protocol": 1,
                    "key_rules": 1
                }),
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(
                serde_json::to_value(&event).expect("serialize startup event"),
                expected
            );
        }
    }
}

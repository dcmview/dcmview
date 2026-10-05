//! The session's bearer token and the check every `/api` request passes.
//!
//! One token per process, valid for its lifetime: the process is ephemeral,
//! so there is no expiry and no rotation. The viewer page and its hashed
//! assets are public (they hold the build, no file data); everything under
//! `/api` is not.

use super::error::ApiError;
use anyhow::Result;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::fmt;

/// Bytes of OS randomness in a generated token; 43 characters as unpadded
/// base64url.
pub const TOKEN_BYTES: usize = 32;

/// A bearer token. It is deliberately not `Display`, and `Debug` never
/// prints the value, so it cannot reach a log line by accident.
#[derive(Clone)]
pub struct AccessToken(String);

impl AccessToken {
    /// A fresh token: [`TOKEN_BYTES`] from the OS random source, encoded as
    /// unpadded base64url.
    pub fn generate() -> Result<Self> {
        use base64::Engine;

        let mut bytes = [0; TOKEN_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|_| anyhow::anyhow!("failed to generate access token from OS randomness"))?;
        Ok(Self(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// A token fixed by the owner through `DCMVIEW_TOKEN`. It must be
    /// non-empty and made only of `A-Z a-z 0-9 - . _ ~`, so it is safe
    /// unescaped in a URL fragment and in a header; anything else is an error
    /// that names the variable and never echoes the value.
    pub fn fixed(value: &str) -> Result<Self> {
        anyhow::ensure!(
            !value.is_empty()
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                }),
            "DCMVIEW_TOKEN must be non-empty and contain only A-Z a-z 0-9 - . _ ~"
        );
        Ok(Self(value.to_owned()))
    }

    /// The token text, for the launch URL, the startup line and the header
    /// an integration sends. Call it only where the token is meant to leave
    /// the process.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `presented` is this token, compared in constant time.
    pub fn matches(&self, presented: &str) -> bool {
        use subtle::ConstantTimeEq;

        bool::from(self.0.as_bytes().ct_eq(presented.as_bytes()))
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessToken(..)")
    }
}

/// Answers 401 unless the request carries `Authorization: Bearer <token>`
/// with this session's token. The token is read from that header only: a
/// query parameter would land in logs and `Referer`, and a cookie is not
/// sent to the VS Code webview's cross-site frame.
pub(super) async fn require_bearer(
    State(token): State<AccessToken>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(scheme, value)| {
            scheme.eq_ignore_ascii_case(crate::api::contracts::AUTHORIZATION_SCHEME)
                && token.matches(value.trim_start_matches(' '))
        });
    if !authorized {
        return Err(ApiError::unauthorized());
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::{AccessToken, TOKEN_BYTES};

    #[test]
    fn generated_tokens_are_unpadded_base64url_and_differ() {
        let first = AccessToken::generate().expect("generate token");
        let second = AccessToken::generate().expect("generate token");

        // 32 bytes are 43 unpadded base64 characters.
        assert_eq!(first.expose().len(), (TOKEN_BYTES * 4).div_ceil(3));
        assert!(first
            .expose()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
        assert_ne!(first.expose(), second.expose());
        assert!(first.matches(first.expose()));
        assert!(!first.matches(second.expose()));
    }

    #[test]
    fn fixed_tokens_must_be_safe_in_a_fragment_and_a_header() {
        for accepted in [
            "a",
            "Xy-_.~09",
            "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG",
        ] {
            let token = AccessToken::fixed(accepted).expect("accepted token");
            assert_eq!(token.expose(), accepted);
        }
        for rejected in [
            "",
            " ",
            "has space",
            "a#b",
            "a&b",
            "a/b",
            "a=b",
            "tok\n",
            "é",
        ] {
            assert!(
                AccessToken::fixed(rejected).is_err(),
                "{rejected:?} should be rejected"
            );
        }
    }

    #[test]
    fn debug_output_never_contains_the_token() {
        let token = AccessToken::fixed("super-secret-value").expect("token");
        assert!(!format!("{token:?}").contains("super-secret-value"));
    }
}

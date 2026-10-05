//! The bearer token boundary: everything under `/api` needs the session's
//! token, the viewer page and its assets do not.

use super::support;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum_test::TestServer;
use dcmview::server::{self, AccessToken};
use serde_json::Value;

const TOKEN: &str = "test-token_0123456789.~";

fn server_with_token() -> TestServer {
    let token = AccessToken::fixed(TOKEN).expect("test token");
    TestServer::new(server::router(
        support::app_state(vec![]).with_access_token(token),
    ))
}

#[tokio::test]
async fn api_requests_without_the_session_token_are_401() {
    let server = server_with_token();
    let bearer = |value: &str| Some(format!("Bearer {value}"));
    // (method, path, Authorization header)
    let cases: [(Method, &str, Option<String>); 8] = [
        (Method::GET, "/api/health", None),
        (Method::GET, "/api/files", bearer("not-the-token")),
        (Method::GET, "/api/files", bearer("")),
        (Method::GET, "/api/files", Some(format!("Basic {TOKEN}"))),
        (Method::GET, "/api/files", Some(TOKEN.to_string())),
        // The token is accepted in the header only, never in the query.
        (
            Method::GET,
            "/api/files?token=test-token_0123456789.~",
            None,
        ),
        // A missing route must not be told apart from a declared one.
        (Method::GET, "/api/no-such-route", None),
        (Method::PUT, "/api/file/0/annotations", None),
    ];

    for (method, path, authorization) in cases {
        let mut request = server.method(method.clone(), path);
        if let Some(value) = &authorization {
            request = request.add_header(
                header::AUTHORIZATION,
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        let response = request.await;
        let case = format!("{method} {path} with {authorization:?}");

        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED, "{case}");
        assert_eq!(
            response.header(header::WWW_AUTHENTICATE),
            "Bearer",
            "{case}"
        );
        assert!(
            response.maybe_header("X-Server-Instance").is_some(),
            "{case}"
        );
        let body: Value = response.json();
        assert_eq!(body["code"], "unauthorized", "{case}");
        assert!(body["error"].is_string(), "{case}");
    }
}

#[tokio::test]
async fn the_session_token_opens_the_api() {
    let server = server_with_token();

    for path in ["/api/health", "/api/files"] {
        server
            .get(path)
            .add_header(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {TOKEN}")).expect("header value"),
            )
            .await
            .assert_status_ok();
    }

    // The scheme is case-insensitive (RFC 9110 section 11.1).
    server
        .get("/api/health")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("bearer {TOKEN}")).expect("header value"),
        )
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn the_viewer_page_and_assets_need_no_token() {
    let server = server_with_token();

    let page = server.get("/").await;
    page.assert_status_ok();
    assert!(page
        .header(header::CONTENT_TYPE)
        .to_str()
        .expect("content type")
        .starts_with("text/html"));

    // A missing asset is a plain 404, not a 401: assets are outside the API.
    assert_eq!(
        server.get("/assets/no-such-asset.js").await.status_code(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_server_without_a_token_keeps_the_api_open() {
    let server = TestServer::new(server::router(support::app_state(vec![])));

    server.get("/api/health").await.assert_status_ok();
}

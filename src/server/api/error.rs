use crate::api::contracts::{ApiErrorCode, ErrorResponse, UNAUTHORIZED_CHALLENGE};
use crate::pixels::{self, PixelError};
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug)]
pub(super) struct ApiError {
    status: StatusCode,
    code: ApiErrorCode,
    message: String,
}

impl ApiError {
    fn from_rejection(status: StatusCode, code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: ApiErrorCode::BadRequest,
            message: message.into(),
        }
    }

    /// A well-formed query whose value is outside what the endpoint takes.
    pub(super) fn invalid_query(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::BAD_REQUEST, ApiErrorCode::InvalidQuery, message)
    }

    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: ApiErrorCode::NotFound,
            message: message.into(),
        }
    }

    fn method_not_allowed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::METHOD_NOT_ALLOWED,
            code: ApiErrorCode::MethodNotAllowed,
            message: message.into(),
        }
    }

    pub(super) fn masked(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::FORBIDDEN, ApiErrorCode::Masked, message)
    }

    /// The one answer for a missing, malformed or wrong token: the response
    /// does not say which, and does not say whether the route exists.
    pub(super) fn unauthorized() -> Self {
        Self::coded(
            StatusCode::UNAUTHORIZED,
            ApiErrorCode::Unauthorized,
            "missing or invalid access token: send Authorization: Bearer <token>",
        )
    }

    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: ApiErrorCode::InternalError,
            message: message.into(),
        }
    }

    pub(super) fn semantic_mapping_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: ApiErrorCode::SemanticMappingUnavailable,
            message: message.into(),
        }
    }

    pub(super) fn overlay_not_covering_frame(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: ApiErrorCode::OverlayNotCoveringFrame,
            message: message.into(),
        }
    }

    fn coded(status: StatusCode, code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

pub(super) fn path_rejection(error: PathRejection) -> ApiError {
    // axum's text names Rust types and parser internals.
    ApiError::from_rejection(
        error.status(),
        ApiErrorCode::InvalidPath,
        "invalid path: file indexes and frame numbers are non-negative integers",
    )
}

/// A read that failed because the file was deleted or moved after discovery
/// is a 404 that says so, rather than a 500 with the system's message.
pub(super) fn gone_or(path: &std::path::Path, error: ApiError) -> ApiError {
    if error.status.is_server_error() && !path.exists() {
        return ApiError::not_found(format!(
            "{} no longer exists; restart dcmview to scan again",
            path.display()
        ));
    }
    error
}

pub(super) fn query_rejection(error: QueryRejection) -> ApiError {
    ApiError::from_rejection(
        error.status(),
        ApiErrorCode::InvalidQuery,
        error.body_text(),
    )
}

pub(super) fn json_rejection(error: JsonRejection) -> ApiError {
    ApiError::from_rejection(error.status(), ApiErrorCode::InvalidJson, error.body_text())
}

pub(super) fn pixel_error(error: PixelError) -> ApiError {
    match error {
        pixels::PixelError::NoPixelData { .. } => ApiError::coded(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NoPixelData,
            error.to_string(),
        ),
        pixels::PixelError::FrameOutOfRange { .. } => ApiError::coded(
            StatusCode::NOT_FOUND,
            ApiErrorCode::FrameOutOfRange,
            error.to_string(),
        ),
        pixels::PixelError::InvalidWindow(_) => ApiError::coded(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::InvalidWindow,
            error.to_string(),
        ),
        pixels::PixelError::UnsupportedTransferSyntax(_) => ApiError::coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::UnsupportedTransferSyntax,
            error.to_string(),
        ),
        pixels::PixelError::UnsupportedLayout(_) => ApiError::coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::UnsupportedPixelLayout,
            error.to_string(),
        ),
        pixels::PixelError::Decode { .. } => ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::PixelDecodeFailed,
            error.to_string(),
        ),
    }
}

pub(super) async fn not_found_handler() -> ApiError {
    ApiError::coded(
        StatusCode::NOT_FOUND,
        ApiErrorCode::RouteNotFound,
        "API route not found",
    )
}

/// Outside `/api`: only the viewer page and its assets are served.
pub(super) async fn page_not_found_handler() -> ApiError {
    ApiError::coded(
        StatusCode::NOT_FOUND,
        ApiErrorCode::RouteNotFound,
        "not found: the viewer is served at /",
    )
}

pub(super) async fn method_not_allowed_handler() -> ApiError {
    ApiError::method_not_allowed("method not allowed")
}

/// A server error's message, kept on the response so the request logger can
/// report it with the request that caused it.
#[derive(Clone)]
pub(super) struct ServerErrorMessage(pub(super) String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let logged = self
            .status
            .is_server_error()
            .then(|| ServerErrorMessage(self.message.clone()));
        let mut response = (
            self.status,
            Json(ErrorResponse {
                code: self.code,
                error: self.message,
            }),
        )
            .into_response();
        if self.code == ApiErrorCode::Unauthorized {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(UNAUTHORIZED_CHALLENGE),
            );
        }
        if let Some(message) = logged {
            response.extensions_mut().insert(message);
        }
        response
    }
}

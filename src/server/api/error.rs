use crate::api::contracts::{ApiErrorCode, ErrorResponse};
use crate::pixels::{self, PixelError};
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
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
    ApiError::from_rejection(error.status(), ApiErrorCode::InvalidPath, error.body_text())
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
        pixels::PixelError::NoPixelData => ApiError::coded(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NoPixelData,
            error.to_string(),
        ),
        pixels::PixelError::FrameOutOfRange => ApiError::coded(
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

pub(super) async fn method_not_allowed_handler() -> ApiError {
    ApiError::method_not_allowed("method not allowed")
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                code: self.code,
                error: self.message,
            }),
        )
            .into_response()
    }
}

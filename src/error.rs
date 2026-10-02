use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Error API yang otomatis dirender sebagai JSON `{ "error": "..." }`.
#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Conflict(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

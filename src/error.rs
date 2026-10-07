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
    /// Kegagalan internal (mis. database). Detail hanya dicatat di log, tidak dikirim ke klien.
    Internal(String),
}

impl From<tokio_postgres::Error> for ApiError {
    fn from(err: tokio_postgres::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Internal(detail) => {
                eprintln!("kesalahan internal: {detail}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "terjadi kesalahan pada server, coba lagi".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

//! Header keamanan untuk semua response (dicek oleh unit test & OWASP ZAP di pipeline).

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// Semua asset dilayani dari origin sendiri, tanpa inline script/style.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
                   frame-ancestors 'none'";

const HEADERS: [(HeaderName, &str); 7] = [
    (header::CONTENT_SECURITY_POLICY, CSP),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::X_FRAME_OPTIONS, "DENY"),
    (header::REFERRER_POLICY, "no-referrer"),
    (
        HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()",
    ),
    (
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    ),
    (
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    ),
];

pub async fn headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let response_headers = response.headers_mut();
    for (name, value) in HEADERS {
        response_headers.insert(name, HeaderValue::from_static(value));
    }
    // Data API selalu fresh; asset statis mengatur cache-nya sendiri.
    response_headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

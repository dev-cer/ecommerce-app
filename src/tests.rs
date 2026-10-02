//! Test API end-to-end lewat router (tanpa membuka port): fungsional, keamanan, dan konkurensi.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{routes, state::AppState};

fn app() -> (Router, AppState) {
    let state = AppState::new();
    (routes::router(state.clone()), state)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    (
        parts.status,
        parts.headers,
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn post_json(body: impl Into<Body>) -> Request<Body> {
    Request::post("/api/orders")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .unwrap()
}

fn order(customer: &str, items: Value) -> Request<Body> {
    post_json(json!({ "customer": customer, "items": items }).to_string())
}

async fn status_of(app: &Router, req: Request<Body>) -> StatusCode {
    send(app, req).await.0
}

// ---------- Fungsional ----------

#[tokio::test]
async fn storefront_and_assets_are_served() {
    let (app, _) = app();
    for (uri, content_type) in [
        ("/", "text/html"),
        ("/app.css", "text/css"),
        ("/app.js", "text/javascript"),
    ] {
        let (status, headers, _) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with(content_type),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn successful_order_reduces_stock_and_updates_stats() {
    let (app, state) = app();
    let before = state.product(1).unwrap().stock;

    let (status, _, body) = send(
        &app,
        order("Budi", json!([{ "product_id": 1, "quantity": 2 }])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let order: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(order["total"], 178_000);
    assert_eq!(state.product(1).unwrap().stock, before - 2);
    assert_eq!(state.stats().orders, 1);
}

#[tokio::test]
async fn metrics_expose_business_and_http_series() {
    let (app, _) = app();
    send(&app, get("/api/products")).await;
    let (_, _, body) = send(&app, get("/metrics")).await;
    let text = String::from_utf8(body).unwrap();
    for series in [
        "nusamart_orders_total 0",
        "nusamart_http_requests_total{class=\"2xx\"} 1",
        "nusamart_http_request_duration_seconds_bucket{le=\"+Inf\"} 1",
        "nusamart_http_request_duration_seconds_count 1",
    ] {
        assert!(text.contains(series), "missing `{series}` in:\n{text}");
    }
}

#[tokio::test]
async fn restock_refills_to_initial_stock() {
    let (app, state) = app();
    let initial = state.stats().stock_units;
    send(
        &app,
        order("Budi", json!([{ "product_id": 9, "quantity": 6 }])),
    )
    .await;
    assert_eq!(state.product(9).unwrap().stock, 0);

    assert_eq!(state.restock(), 6);
    assert_eq!(state.stats().stock_units, initial);
    assert_eq!(state.restock(), 0, "restock kedua tidak menambah apa-apa");
}

// ---------- Keamanan ----------

#[tokio::test]
async fn security_headers_on_every_response() {
    let (app, _) = app();
    for uri in ["/", "/api/products", "/does-not-exist"] {
        let (_, headers, _) = send(&app, get(uri)).await;
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(
            csp.contains("default-src 'self'") && !csp.contains("unsafe-inline"),
            "{uri}: {csp}"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{uri}");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY", "{uri}");
        assert!(headers.contains_key("permissions-policy"), "{uri}");
    }
}

#[tokio::test]
async fn rejects_invalid_orders() {
    let (app, _) = app();
    let long_name = "A".repeat(61);
    let too_many_lines: Vec<_> = (0..21)
        .map(|_| json!({ "product_id": 7, "quantity": 1 }))
        .collect();
    let cases = [
        (
            "nama kosong",
            order("   ", json!([{ "product_id": 1, "quantity": 1 }])),
            StatusCode::BAD_REQUEST,
        ),
        (
            "nama terlalu panjang",
            order(&long_name, json!([{ "product_id": 1, "quantity": 1 }])),
            StatusCode::BAD_REQUEST,
        ),
        (
            "karakter kontrol",
            order("Budi\u{0}\n", json!([{ "product_id": 1, "quantity": 1 }])),
            StatusCode::BAD_REQUEST,
        ),
        (
            "keranjang kosong",
            order("Budi", json!([])),
            StatusCode::BAD_REQUEST,
        ),
        (
            "jumlah nol",
            order("Budi", json!([{ "product_id": 1, "quantity": 0 }])),
            StatusCode::BAD_REQUEST,
        ),
        (
            "terlalu banyak baris",
            order("Budi", json!(too_many_lines)),
            StatusCode::BAD_REQUEST,
        ),
        (
            "produk tidak ada",
            order("Budi", json!([{ "product_id": 999, "quantity": 1 }])),
            StatusCode::NOT_FOUND,
        ),
        (
            "melebihi stok",
            order("Budi", json!([{ "product_id": 9, "quantity": 7 }])),
            StatusCode::CONFLICT,
        ),
    ];
    for (name, req, expected) in cases {
        assert_eq!(status_of(&app, req).await, expected, "{name}");
    }
}

#[tokio::test]
async fn quantity_overflow_is_rejected_not_panicking() {
    let (app, state) = app();
    let max = u32::MAX;
    let req = order(
        "Eve",
        json!([{ "product_id": 1, "quantity": max }, { "product_id": 1, "quantity": max }]),
    );
    assert_eq!(status_of(&app, req).await, StatusCode::BAD_REQUEST);
    // Angka negatif / di luar u32 ditolak oleh deserializer.
    let req = order("Eve", json!([{ "product_id": 1, "quantity": -1 }]));
    assert!(status_of(&app, req).await.is_client_error());
    assert_eq!(state.stats().orders, 0);
}

#[tokio::test]
async fn failed_order_is_all_or_nothing() {
    let (app, state) = app();
    let stock_before = state.stats().stock_units;
    // Baris pertama valid, baris kedua melebihi stok → tidak boleh ada stok yang berkurang.
    let req = order(
        "Budi",
        json!([{ "product_id": 1, "quantity": 1 }, { "product_id": 9, "quantity": 50 }]),
    );
    assert!(status_of(&app, req).await.is_client_error());
    assert_eq!(state.stats().stock_units, stock_before);
}

#[tokio::test]
async fn rejects_malformed_and_oversized_payloads() {
    let (app, _) = app();
    assert!(
        status_of(&app, post_json("{not json"))
            .await
            .is_client_error()
    );

    let no_content_type = Request::post("/api/orders").body(Body::from("{}")).unwrap();
    assert_eq!(
        status_of(&app, no_content_type).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let huge = format!(r#"{{"customer":"{}","items":[]}}"#, "x".repeat(64 * 1024));
    assert_eq!(
        status_of(&app, post_json(huge)).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn customer_input_is_returned_as_json_not_html() {
    let (app, _) = app();
    let xss = "<script>alert(1)</script>";
    let (status, headers, _) = send(
        &app,
        order(xss, json!([{ "product_id": 1, "quantity": 1 }])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
}

#[tokio::test]
async fn path_and_query_fuzzing_does_not_crash() {
    let (app, _) = app();
    for uri in [
        "/api/products/abc",
        "/api/products/-1",
        "/api/products/99999999999999999999",
        "/api/products?q=%00%27%22%3Cscript%3E",
        "/api/products?category=../../etc/passwd",
        "/../../etc/passwd",
    ] {
        let status = status_of(&app, get(uri)).await;
        assert!(
            status.is_success() || status.is_client_error(),
            "{uri} → {status}"
        );
    }
}

// ---------- Konkurensi ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_orders_never_oversell() {
    let (app, state) = app();
    let stock = state.product(9).unwrap().stock as usize; // Keyboard: stok 6

    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let app = app.clone();
            tokio::spawn(async move {
                status_of(
                    &app,
                    order(
                        &format!("buyer-{i}"),
                        json!([{ "product_id": 9, "quantity": 1 }]),
                    ),
                )
                .await
            })
        })
        .collect();

    let mut created = 0;
    for task in tasks {
        match task.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::CONFLICT => {}
            other => panic!("status tak terduga: {other}"),
        }
    }
    assert_eq!(created, stock);
    assert_eq!(state.product(9).unwrap().stock, 0);
}

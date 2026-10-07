//! Test API end-to-end lewat router (tanpa membuka port): fungsional dan keamanan.
//! `postgres_store_serves_the_same_catalog` juga menguji PostgreSQL bila `TEST_DATABASE_URL` di-set.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::{routes, state::AppState, store::Store};

/// Beban rekomendasi kecil agar test cepat; logikanya sama dengan produksi.
fn app() -> Router {
    routes::router(AppState::new(Store::memory(), 1_000))
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes().to_vec();
    (parts.status, parts.headers, bytes)
}

async fn json(app: &Router, uri: &str) -> Value {
    let (status, _, body) = send(app, get(uri)).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    serde_json::from_slice(&body).unwrap()
}

// ---------- Fungsional ----------

#[tokio::test]
async fn storefront_and_assets_are_served() {
    let app = app();
    for (uri, content_type) in [
        ("/", "text/html"),
        ("/app.css", "text/css"),
        ("/app.js", "text/javascript"),
    ] {
        let (status, headers, _) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let actual = headers[header::CONTENT_TYPE].to_str().unwrap();
        assert!(actual.starts_with(content_type), "{uri}: {actual}");
    }
}

#[tokio::test]
async fn health_and_readiness() {
    let app = app();
    let health = json(&app, "/health").await;
    assert_eq!(health["status"], "ok");
    assert!(health["build"].is_string() && health["instance"].is_string());
    assert_eq!(json(&app, "/ready").await["status"], "ready");
}

#[tokio::test]
async fn catalog_search_and_category_filter() {
    let app = app();
    let all = json(&app, "/api/products").await;
    assert_eq!(all.as_array().unwrap().len(), 12);

    let kopi = json(&app, "/api/products?q=KOPI").await;
    assert_eq!(kopi.as_array().unwrap().len(), 1);
    assert_eq!(kopi[0]["id"], 1);

    let food = json(&app, "/api/products?category=Makanan").await;
    assert!(
        food.as_array()
            .unwrap()
            .iter()
            .all(|p| p["category"] == "Makanan")
    );
    assert_eq!(
        json(&app, "/api/products?q=tidak-ada").await,
        Value::Array(vec![])
    );
}

#[tokio::test]
async fn live_stats_unavailable_without_prometheus() {
    // Test tidak men-set PROMETHEUS_URL: storefront harus bisa fallback ke statistik katalog.
    let (status, _, body) = send(&app(), get("/api/live")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn product_detail_and_not_found() {
    let app = app();
    assert_eq!(json(&app, "/api/products/2").await["id"], 2);
    let (status, headers, body) = send(&app, get("/api/products/999")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert!(error["error"].as_str().unwrap().contains("999"));
}

#[tokio::test]
async fn recommendations_exclude_product_and_prefer_same_category() {
    let app = app();
    let recs = json(&app, "/api/recommendations?product_id=1").await;
    let recs = recs.as_array().unwrap();
    assert_eq!(recs.len(), 4);
    assert!(recs.iter().all(|r| r["product_id"] != 1));
    // Produk 1 = Makanan; produk Makanan lain (4, 7) harus berada di urutan teratas.
    let top: Vec<_> = recs[..2]
        .iter()
        .map(|r| r["product_id"].as_i64().unwrap())
        .collect();
    assert!(top.contains(&4) && top.contains(&7), "top: {top:?}");

    let (status, _, _) = send(&app, get("/api/recommendations?product_id=999")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, get("/api/recommendations")).await;
    assert!(status.is_client_error());
}

#[tokio::test]
async fn metrics_expose_build_and_http_series() {
    let app = app();
    send(&app, get("/api/products")).await;
    send(&app, get("/api/products/999")).await;
    let (_, _, body) = send(&app, get("/metrics")).await;
    let text = String::from_utf8(body).unwrap();
    for series in [
        "nusamart_build_info{",
        "nusamart_http_requests_total{class=\"2xx\"} 1",
        "nusamart_http_requests_total{class=\"4xx\"} 1",
        "nusamart_http_request_duration_seconds_count 2",
    ] {
        assert!(text.contains(series), "tidak ada `{series}` di:\n{text}");
    }
}

// ---------- Keamanan ----------

#[tokio::test]
async fn security_headers_on_every_response() {
    let app = app();
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
async fn write_methods_are_not_allowed() {
    let app = app();
    for uri in ["/api/products", "/api/products/1", "/health"] {
        let req = Request::post(uri).body(Body::from("{}")).unwrap();
        let (status, _, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{uri}");
    }
}

#[tokio::test]
async fn path_and_query_fuzzing_does_not_crash() {
    let app = app();
    for uri in [
        "/api/products/abc",
        "/api/products/-1",
        "/api/products/99999999999999999999",
        "/api/products?q=%00%27%22%3Cscript%3E",
        "/api/products?q=%27%20OR%201%3D1--",
        "/api/products?category=../../etc/passwd",
        "/api/recommendations?product_id=-2147483648",
        "/../../etc/passwd",
    ] {
        let (status, _, _) = send(&app, get(uri)).await;
        assert!(
            status.is_success() || status.is_client_error(),
            "{uri} → {status}"
        );
    }
}

// ---------- PostgreSQL ----------

#[tokio::test]
async fn postgres_store_serves_the_same_catalog() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL tidak di-set: test PostgreSQL dilewati");
        return;
    };
    // Dua kali: startup kedua (pod lain / restart) tidak boleh men-seed ulang atau gagal.
    Store::postgres(&url, 30).await.unwrap();
    let app = routes::router(AppState::new(
        Store::postgres(&url, 30).await.unwrap(),
        1_000,
    ));

    assert_eq!(json(&app, "/health").await["storage"], "postgres");
    assert_eq!(json(&app, "/ready").await["status"], "ready");
    assert_eq!(
        json(&app, "/api/products").await.as_array().unwrap().len(),
        12
    );
    assert_eq!(json(&app, "/api/products?q=KOPI").await[0]["id"], 1);
    assert_eq!(
        json(&app, "/api/products/2").await["name"],
        "Batik Tulis Pekalongan"
    );
    let (status, _, _) = send(&app, get("/api/products/999")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Input berbahaya hanya dianggap teks pencarian (query berparameter).
    let injected = json(&app, "/api/products?q=%27%20OR%201%3D1--").await;
    assert_eq!(injected, Value::Array(vec![]));
}

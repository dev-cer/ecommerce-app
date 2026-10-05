//! Test API end-to-end lewat router (tanpa membuka port): fungsional, keamanan, dan konkurensi.
//!
//! Setiap test dijalankan untuk backend in-memory, dan juga PostgreSQL bila `TEST_DATABASE_URL`
//! di-set (mis. `postgres://postgres@localhost:5432/postgres`). Tiap test PostgreSQL memakai
//! database baru yang dihapus lagi setelah test selesai.

use std::sync::atomic::{AtomicU32, Ordering};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection, postgres::PgConnectOptions};
use tower::ServiceExt;

use crate::{
    routes,
    state::AppState,
    store::{MemoryStore, PgStore, Store},
};

struct Backend {
    label: &'static str,
    app: Router,
    state: AppState,
    /// Database sementara (khusus PostgreSQL) beserta koneksi admin untuk membuatnya.
    pg: Option<(PgConnectOptions, String)>,
}

impl Backend {
    fn new(label: &'static str, store: Store, pg: Option<(PgConnectOptions, String)>) -> Self {
        let state = AppState::new(store);
        Self {
            label,
            app: routes::router(state.clone()),
            state,
            pg,
        }
    }

    /// Replika kedua yang berbagi database yang sama (mensimulasikan 2 pod di belakang Service).
    async fn second_replica(&self) -> Option<Backend> {
        let (admin, db) = self.pg.as_ref()?;
        let store = PgStore::connect_with(admin.clone().database(db), 5, 1)
            .await
            .unwrap();
        Some(Backend::new(
            "postgres-replica-2",
            Store::Postgres(store),
            None,
        ))
    }

    async fn cleanup(self) {
        let Some((admin, db)) = self.pg.clone() else {
            return;
        };
        drop(self);
        let mut conn = PgConnection::connect_with(&admin).await.unwrap();
        let drop_db = format!(r#"DROP DATABASE IF EXISTS "{db}" WITH (FORCE)"#);
        let _ = conn.execute(AssertSqlSafe(drop_db)).await;
    }
}

async fn backends() -> Vec<Backend> {
    let mut backends = vec![Backend::new(
        "memory",
        Store::Memory(MemoryStore::new()),
        None,
    )];

    if let Ok(url) = std::env::var("TEST_DATABASE_URL") {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let db = format!(
            "nusamart_test_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let admin: PgConnectOptions = url.parse().expect("TEST_DATABASE_URL tidak valid");
        let mut conn = PgConnection::connect_with(&admin)
            .await
            .expect("tidak bisa terhubung ke TEST_DATABASE_URL");
        // Nama database dibuat sendiri oleh test (bukan input pengguna), jadi aman disisipkan.
        conn.execute(AssertSqlSafe(format!(r#"CREATE DATABASE "{db}""#)))
            .await
            .unwrap();
        let store = PgStore::connect_with(admin.clone().database(&db), 10, 1)
            .await
            .unwrap();
        backends.push(Backend::new(
            "postgres",
            Store::Postgres(store),
            Some((admin, db)),
        ));
    }
    backends
}

/// Jalankan isi test untuk setiap backend yang tersedia.
macro_rules! backend_test {
    ($(#[$meta:meta])* $name:ident, |$app:ident, $state:ident| $body:block) => {
        $(#[$meta])*
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn $name() {
            for backend in backends().await {
                eprintln!("backend: {}", backend.label);
                {
                    let $app = backend.app.clone();
                    let $state = backend.state.clone();
                    let _ = (&$app, &$state);
                    async { $body }.await;
                }
                backend.cleanup().await;
            }
        }
    };
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

backend_test!(storefront_and_assets_are_served, |app, _state| {
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
});

backend_test!(health_and_readiness, |app, state| {
    let (_, _, body) = send(&app, get("/health")).await;
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["storage"], state.store.kind());
    assert_eq!(status_of(&app, get("/ready")).await, StatusCode::OK);
});

backend_test!(
    successful_order_reduces_stock_and_updates_stats,
    |app, state| {
        let before = state.product(1).await.unwrap().stock;

        let req = order("Budi", json!([{ "product_id": 1, "quantity": 2 }]));
        let (status, _, body) = send(&app, req).await;
        assert_eq!(status, StatusCode::CREATED);
        let created: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(created["id"], "NM-00001");
        assert_eq!(created["total"], 178_000);
        assert_eq!(state.product(1).await.unwrap().stock, before - 2);

        let stats = state.stats().await.unwrap();
        assert_eq!(
            (stats.orders, stats.items_sold, stats.revenue),
            (1, 2, 178_000)
        );

        let (_, _, body) = send(&app, get("/api/orders")).await;
        let orders: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(orders[0]["items"][0]["quantity"], 2);
    }
);

backend_test!(duplicate_lines_are_merged_per_product, |app, state| {
    let req = order(
        "Budi",
        json!([{ "product_id": 1, "quantity": 1 }, { "product_id": 1, "quantity": 2 }]),
    );
    let (status, _, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::CREATED);
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(created["items"].as_array().unwrap().len(), 1);
    assert_eq!(created["items"][0]["quantity"], 3);
    assert_eq!(state.stats().await.unwrap().items_sold, 3);
});

backend_test!(metrics_expose_business_and_http_series, |app, state| {
    send(&app, get("/api/products")).await;
    let (_, _, body) = send(&app, get("/metrics")).await;
    let text = String::from_utf8(body).unwrap();
    let storage = format!("storage=\"{}\"", state.store.kind());
    for series in [
        "nusamart_orders_total 0",
        "nusamart_http_requests_total{class=\"2xx\"} 1",
        "nusamart_http_request_duration_seconds_bucket{le=\"+Inf\"} 1",
        "nusamart_http_request_duration_seconds_count 1",
        storage.as_str(),
    ] {
        assert!(text.contains(series), "missing `{series}` in:\n{text}");
    }
});

backend_test!(restock_refills_to_initial_stock, |app, state| {
    let initial = state.stats().await.unwrap().stock_units;
    send(
        &app,
        order("Budi", json!([{ "product_id": 9, "quantity": 6 }])),
    )
    .await;
    assert_eq!(state.product(9).await.unwrap().stock, 0);

    assert_eq!(state.store.restock().await.unwrap(), 6);
    assert_eq!(state.stats().await.unwrap().stock_units, initial);
    assert_eq!(
        state.store.restock().await.unwrap(),
        0,
        "restock kedua tidak menambah apa-apa"
    );
});

// ---------- Keamanan ----------

backend_test!(security_headers_on_every_response, |app, _state| {
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
});

backend_test!(rejects_invalid_orders, |app, _state| {
    let one = json!([{ "product_id": 1, "quantity": 1 }]);
    let too_many_lines: Vec<_> = (0..21)
        .map(|_| json!({ "product_id": 7, "quantity": 1 }))
        .collect();
    let cases = [
        (
            "nama kosong",
            order("   ", one.clone()),
            StatusCode::BAD_REQUEST,
        ),
        (
            "nama terlalu panjang",
            order(&"A".repeat(61), one.clone()),
            StatusCode::BAD_REQUEST,
        ),
        (
            "karakter kontrol",
            order("Budi\u{0}\n", one.clone()),
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
});

backend_test!(quantity_overflow_is_rejected_not_panicking, |app, state| {
    let max = u32::MAX;
    let req = order(
        "Eve",
        json!([{ "product_id": 1, "quantity": max }, { "product_id": 1, "quantity": max }]),
    );
    assert_eq!(status_of(&app, req).await, StatusCode::BAD_REQUEST);
    // Angka negatif / di luar u32 ditolak oleh deserializer.
    let req = order("Eve", json!([{ "product_id": 1, "quantity": -1 }]));
    assert!(status_of(&app, req).await.is_client_error());
    assert_eq!(state.stats().await.unwrap().orders, 0);
});

backend_test!(failed_order_is_all_or_nothing, |app, state| {
    let before = state.stats().await.unwrap();
    // Baris pertama valid, baris kedua melebihi stok → tidak boleh ada stok/order yang tercatat.
    let req = order(
        "Budi",
        json!([{ "product_id": 1, "quantity": 1 }, { "product_id": 9, "quantity": 50 }]),
    );
    assert_eq!(status_of(&app, req).await, StatusCode::CONFLICT);
    let after = state.stats().await.unwrap();
    assert_eq!(
        (after.stock_units, after.orders),
        (before.stock_units, before.orders)
    );
});

backend_test!(rejects_malformed_and_oversized_payloads, |app, _state| {
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
});

backend_test!(
    sql_injection_payloads_are_stored_as_plain_data,
    |app, state| {
        let payload = "Robert'); DROP TABLE orders;--";
        let (status, _, body) = send(
            &app,
            order(payload, json!([{ "product_id": 1, "quantity": 1 }])),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let created: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(created["customer"], payload);
        // Tabel masih utuh dan order tercatat.
        assert_eq!(state.stats().await.unwrap().orders, 1);
        let status = status_of(&app, get("/api/products?q=%27%20OR%201%3D1--")).await;
        assert_eq!(status, StatusCode::OK);
    }
);

backend_test!(
    customer_input_is_returned_as_json_not_html,
    |app, _state| {
        let xss = "<script>alert(1)</script>";
        let (status, headers, _) = send(
            &app,
            order(xss, json!([{ "product_id": 1, "quantity": 1 }])),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let content_type = headers[header::CONTENT_TYPE].to_str().unwrap();
        assert!(content_type.starts_with("application/json"));
    }
);

backend_test!(path_and_query_fuzzing_does_not_crash, |app, _state| {
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
});

// ---------- Konkurensi ----------

/// Tembakkan `count` order paralel untuk produk 9 (stok 6), dibagi rata ke semua `apps`.
async fn order_storm(apps: &[Router], count: usize) -> usize {
    let tasks: Vec<_> = (0..count)
        .map(|i| {
            let app = apps[i % apps.len()].clone();
            tokio::spawn(async move {
                let req = order(
                    &format!("buyer-{i}"),
                    json!([{ "product_id": 9, "quantity": 1 }]),
                );
                status_of(&app, req).await
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
    created
}

backend_test!(concurrent_orders_never_oversell, |app, state| {
    let stock = state.product(9).await.unwrap().stock as usize; // Keyboard: stok 6
    assert_eq!(order_storm(std::slice::from_ref(&app), 50).await, stock);
    assert_eq!(state.product(9).await.unwrap().stock, 0);
});

/// Inti alasan memakai PostgreSQL: dua replika (dua pool koneksi) yang melayani order
/// bersamaan tetap tidak pernah menjual melebihi stok.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_replicas_never_oversell() {
    for backend in backends().await {
        let Some(replica) = backend.second_replica().await else {
            continue; // hanya relevan untuk PostgreSQL
        };
        let stock = backend.state.product(9).await.unwrap().stock as usize;
        let created = order_storm(&[backend.app.clone(), replica.app.clone()], 60).await;
        assert_eq!(created, stock);
        assert_eq!(replica.state.product(9).await.unwrap().stock, 0);
        assert_eq!(backend.state.stats().await.unwrap().orders, stock as i64);
        drop(replica);
        backend.cleanup().await;
    }
}

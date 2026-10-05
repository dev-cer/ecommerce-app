use std::time::Duration;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{StatusCode, header},
    middleware,
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::timeout::TimeoutLayer;

use crate::{
    catalog::Product,
    error::{ApiError, ApiResult},
    metrics,
    recommend::{self, Recommendation},
    security,
    state::{AppState, OrderRequest},
    store::{Order, Stats},
};

/// Ukuran body request maksimum; order terbesar yang valid jauh di bawah ini.
const MAX_BODY_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/products", get(list_products))
        .route("/products/{id}", get(get_product))
        .route("/orders", get(list_orders).post(create_order))
        .route("/stats", get(stats))
        .route("/recommendations", get(recommendations));

    Router::new()
        .route("/", get(storefront))
        .route("/app.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics))
        .nest("/api", api)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(middleware::from_fn(security::headers))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            metrics::track,
        ))
        .with_state(state)
}

async fn storefront() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn stylesheet() -> impl IntoResponse {
    static_asset("text/css; charset=utf-8", include_str!("../static/app.css"))
}

async fn script() -> impl IntoResponse {
    static_asset(
        "text/javascript; charset=utf-8",
        include_str!("../static/app.js"),
    )
}

fn static_asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
}

/// Liveness: proses hidup. Tidak bergantung pada database agar pod tidak di-restart saat DB lambat.
async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "build": &*state.build,
        "instance": &*state.instance,
        "storage": state.store.kind(),
        "uptime_seconds": state.started_at.elapsed().as_secs(),
    }))
}

/// Readiness: pod hanya menerima traffic jika penyimpanan terjangkau.
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    if state.store.ready().await {
        (StatusCode::OK, Json(json!({ "status": "ready" })))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "database tidak terjangkau" })),
        )
    }
}

async fn metrics(State(state): State<AppState>) -> String {
    metrics::render(&state).await
}

#[derive(Deserialize)]
struct ProductFilter {
    category: Option<String>,
    q: Option<String>,
}

async fn list_products(
    State(state): State<AppState>,
    Query(filter): Query<ProductFilter>,
) -> ApiResult<Json<Vec<Product>>> {
    state
        .products(filter.category.as_deref(), filter.q.as_deref())
        .await
        .map(Json)
}

async fn get_product(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<Product>> {
    state.product(id).await.map(Json)
}

async fn list_orders(State(state): State<AppState>) -> ApiResult<Json<Vec<Order>>> {
    state.orders().await.map(Json)
}

async fn create_order(
    State(state): State<AppState>,
    Json(req): Json<OrderRequest>,
) -> ApiResult<(StatusCode, Json<Order>)> {
    state
        .place_order(req)
        .await
        .map(|order| (StatusCode::CREATED, Json(order)))
}

#[derive(Deserialize)]
struct RecommendationQuery {
    product_id: i32,
}

/// Komputasi CPU berat dijalankan di thread pool blocking agar tidak menahan event loop.
async fn recommendations(
    State(state): State<AppState>,
    Query(query): Query<RecommendationQuery>,
) -> ApiResult<Json<Vec<Recommendation>>> {
    let work = state.reco_work;
    tokio::task::spawn_blocking(move || recommend::recommend(query.product_id, work))
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?
        .map(Json)
}

async fn stats(State(state): State<AppState>) -> ApiResult<Json<Stats>> {
    state.stats().await.map(Json)
}

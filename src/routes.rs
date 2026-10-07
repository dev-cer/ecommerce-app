use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    middleware,
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    catalog::Product,
    error::{ApiError, ApiResult},
    metrics,
    recommend::{self, Recommendation},
    security,
    state::AppState,
};

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/products", get(list_products))
        .route("/products/{id}", get(get_product))
        .route("/recommendations", get(recommendations));

    Router::new()
        .route("/", get(storefront))
        .route("/app.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics))
        .nest("/api", api)
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

/// Liveness: proses hidup, plus info build & pod untuk footer storefront.
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

/// Readiness: pod hanya menerima traffic jika database terjangkau.
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
    metrics::render(&state)
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
    let products = state
        .store
        .search(filter.category.as_deref(), filter.q.as_deref())
        .await?;
    Ok(Json(products))
}

async fn get_product(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<Product>> {
    state
        .store
        .product(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("produk #{id} tidak ditemukan")))
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

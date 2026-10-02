use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Html,
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    catalog::Product,
    error::ApiResult,
    state::{AppState, Order, OrderRequest, Stats},
};

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/products", get(list_products))
        .route("/products/{id}", get(get_product))
        .route("/orders", get(list_orders).post(create_order))
        .route("/stats", get(stats));

    Router::new()
        .route("/", get(storefront))
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .nest("/api", api)
        .with_state(state)
}

async fn storefront() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "build": &*state.build,
        "instance": &*state.instance,
        "uptime_seconds": state.started_at.elapsed().as_secs(),
    }))
}

/// Metrik format Prometheus untuk dashboard Grafana.
async fn metrics(State(state): State<AppState>) -> String {
    let s = state.stats();
    let uptime = state.started_at.elapsed().as_secs();
    let build_labels = format!(
        "{{build=\"{}\",version=\"{}\"}}",
        state.build,
        env!("CARGO_PKG_VERSION")
    );

    [
        (
            "nusamart_orders_total",
            "counter",
            "Jumlah order yang berhasil.",
            String::new(),
            s.orders.to_string(),
        ),
        (
            "nusamart_items_sold_total",
            "counter",
            "Jumlah item terjual.",
            String::new(),
            s.items_sold.to_string(),
        ),
        (
            "nusamart_revenue_rupiah_total",
            "counter",
            "Total omzet dalam Rupiah.",
            String::new(),
            s.revenue.to_string(),
        ),
        (
            "nusamart_stock_units",
            "gauge",
            "Total unit stok tersisa.",
            String::new(),
            s.stock_units.to_string(),
        ),
        (
            "nusamart_uptime_seconds",
            "gauge",
            "Lama proses berjalan.",
            String::new(),
            uptime.to_string(),
        ),
        (
            "nusamart_build_info",
            "gauge",
            "Build yang sedang berjalan.",
            build_labels,
            "1".into(),
        ),
    ]
    .iter()
    .map(|(name, kind, help, labels, value)| {
        format!("# HELP {name} {help}\n# TYPE {name} {kind}\n{name}{labels} {value}\n")
    })
    .collect()
}

#[derive(Deserialize)]
struct ProductFilter {
    category: Option<String>,
    q: Option<String>,
}

async fn list_products(
    State(state): State<AppState>,
    Query(filter): Query<ProductFilter>,
) -> Json<Vec<Product>> {
    Json(state.products(filter.category.as_deref(), filter.q.as_deref()))
}

async fn get_product(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> ApiResult<Json<Product>> {
    state.product(id).map(Json)
}

async fn list_orders(State(state): State<AppState>) -> Json<Vec<Order>> {
    Json(state.orders())
}

async fn create_order(
    State(state): State<AppState>,
    Json(req): Json<OrderRequest>,
) -> ApiResult<(StatusCode, Json<Order>)> {
    state
        .place_order(req)
        .map(|order| (StatusCode::CREATED, Json(order)))
}

async fn stats(State(state): State<AppState>) -> Json<Stats> {
    Json(state.stats())
}

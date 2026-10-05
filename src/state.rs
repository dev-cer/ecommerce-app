use std::{collections::BTreeMap, sync::Arc, time::Instant};

use serde::Deserialize;

use crate::{
    catalog::Product,
    error::{ApiError, ApiResult},
    metrics::HttpMetrics,
    store::{NewOrder, Order, Stats, Store},
};

/// Batas input untuk mencegah abuse (payload raksasa, overflow jumlah, nama super panjang).
pub const MAX_CUSTOMER_NAME: usize = 60;
pub const MAX_ORDER_LINES: usize = 20;
pub const MAX_QUANTITY: u32 = 99;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub started_at: Instant,
    /// Tag build dari CI (env `APP_VERSION`), tampil di UI agar rollout terlihat oleh audience.
    pub build: Arc<str>,
    /// Nama pod di Kubernetes (env `HOSTNAME`).
    pub instance: Arc<str>,
    pub http: Arc<HttpMetrics>,
    /// Putaran "model" rekomendasi per kandidat (env `RECO_WORK`); kecil secara default untuk test.
    pub reco_work: u32,
}

#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    pub customer: String,
    pub items: Vec<OrderItemRequest>,
}

#[derive(Debug, Deserialize)]
pub struct OrderItemRequest {
    pub product_id: i32,
    pub quantity: u32,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            started_at: Instant::now(),
            build: env_or("APP_VERSION", "dev").into(),
            instance: env_or("HOSTNAME", "local").into(),
            http: Arc::default(),
            reco_work: 1_000,
        }
    }

    pub fn with_reco_work(mut self, work: u32) -> Self {
        self.reco_work = work;
        self
    }

    pub async fn products(
        &self,
        category: Option<&str>,
        query: Option<&str>,
    ) -> ApiResult<Vec<Product>> {
        let query = query.map(str::to_lowercase);
        let mut products = self.store.products().await?;
        products.retain(|p| {
            category.is_none_or(|c| p.category.eq_ignore_ascii_case(c))
                && query.as_deref().is_none_or(|q| {
                    p.name.to_lowercase().contains(q) || p.description.to_lowercase().contains(q)
                })
        });
        Ok(products)
    }

    pub async fn product(&self, id: i32) -> ApiResult<Product> {
        self.store
            .product(id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("produk #{id} tidak ditemukan")))
    }

    pub async fn orders(&self) -> ApiResult<Vec<Order>> {
        self.store.recent_orders().await
    }

    pub async fn stats(&self) -> ApiResult<Stats> {
        self.store.stats().await
    }

    pub async fn place_order(&self, req: OrderRequest) -> ApiResult<Order> {
        let order = validate(req)?;
        let order = self.store.place_order(order).await?;
        tracing::info!(order = %order.id, customer = %order.customer, total = order.total, "order baru masuk");
        Ok(order)
    }
}

/// Aturan bisnis & keamanan input, sama untuk semua backend penyimpanan.
fn validate(req: OrderRequest) -> ApiResult<NewOrder> {
    let customer = req.customer.trim();
    if customer.is_empty() {
        return Err(ApiError::BadRequest("nama pembeli wajib diisi".into()));
    }
    if customer.chars().count() > MAX_CUSTOMER_NAME {
        return Err(ApiError::BadRequest(format!(
            "nama pembeli maksimal {MAX_CUSTOMER_NAME} karakter"
        )));
    }
    if customer.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(
            "nama pembeli mengandung karakter tidak valid".into(),
        ));
    }
    if req.items.is_empty() {
        return Err(ApiError::BadRequest("keranjang masih kosong".into()));
    }
    if req.items.len() > MAX_ORDER_LINES {
        return Err(ApiError::BadRequest(format!(
            "maksimal {MAX_ORDER_LINES} baris item per order"
        )));
    }

    // Gabungkan baris per produk; BTreeMap sekaligus mengurutkan id (anti-deadlock di Postgres).
    let mut lines = BTreeMap::new();
    for item in &req.items {
        if !(1..=MAX_QUANTITY).contains(&item.quantity) {
            return Err(ApiError::BadRequest(format!(
                "jumlah item harus 1 sampai {MAX_QUANTITY}"
            )));
        }
        // Aman dari overflow: maksimal 20 baris × 99.
        *lines.entry(item.product_id).or_insert(0) += item.quantity as i32;
    }

    Ok(NewOrder {
        customer: customer.to_string(),
        lines: lines.into_iter().collect(),
    })
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

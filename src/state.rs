use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    catalog::{self, Product},
    error::{ApiError, ApiResult},
    metrics::HttpMetrics,
};

/// Batas input untuk mencegah abuse (payload raksasa, overflow jumlah, nama super panjang).
pub const MAX_CUSTOMER_NAME: usize = 60;
pub const MAX_ORDER_LINES: usize = 20;
pub const MAX_QUANTITY: u32 = 99;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Mutex<Store>>,
    pub started_at: Instant,
    /// Tag build dari CI (env `APP_VERSION`), tampil di UI agar rollout terlihat oleh audience.
    pub build: Arc<str>,
    /// Nama pod di Kubernetes (env `HOSTNAME`).
    pub instance: Arc<str>,
    pub http: Arc<HttpMetrics>,
}

struct Store {
    products: Vec<Product>,
    orders: Vec<Order>,
}

#[derive(Debug, Deserialize)]
pub struct OrderRequest {
    pub customer: String,
    pub items: Vec<OrderItemRequest>,
}

#[derive(Debug, Deserialize)]
pub struct OrderItemRequest {
    pub product_id: u32,
    pub quantity: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: String,
    pub customer: String,
    pub items: Vec<OrderLine>,
    pub total: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderLine {
    pub product_id: u32,
    pub name: &'static str,
    pub quantity: u32,
    pub subtotal: u64,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub orders: usize,
    pub revenue: u64,
    pub items_sold: u32,
    pub products: usize,
    pub stock_units: u32,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Store {
                products: catalog::seed(),
                orders: Vec::new(),
            })),
            started_at: Instant::now(),
            build: env_or("APP_VERSION", "dev").into(),
            instance: env_or("HOSTNAME", "local").into(),
            http: Arc::default(),
        }
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        // Data demo tetap konsisten walau ada handler yang panic.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn products(&self, category: Option<&str>, query: Option<&str>) -> Vec<Product> {
        let query = query.map(str::to_lowercase);
        self.store()
            .products
            .iter()
            .filter(|p| category.is_none_or(|c| p.category.eq_ignore_ascii_case(c)))
            .filter(|p| {
                query.as_deref().is_none_or(|q| {
                    p.name.to_lowercase().contains(q) || p.description.to_lowercase().contains(q)
                })
            })
            .cloned()
            .collect()
    }

    pub fn product(&self, id: u32) -> ApiResult<Product> {
        self.store()
            .products
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("produk #{id} tidak ditemukan")))
    }

    pub fn orders(&self) -> Vec<Order> {
        self.store().orders.iter().rev().cloned().collect()
    }

    /// Validasi seluruh item dulu, baru kurangi stok — order bersifat all-or-nothing.
    pub fn place_order(&self, req: OrderRequest) -> ApiResult<Order> {
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

        let mut store = self.store();
        let mut lines = Vec::with_capacity(req.items.len());
        for item in &req.items {
            if !(1..=MAX_QUANTITY).contains(&item.quantity) {
                return Err(ApiError::BadRequest(format!(
                    "jumlah item harus 1 sampai {MAX_QUANTITY}"
                )));
            }
            let product = store
                .products
                .iter()
                .find(|p| p.id == item.product_id)
                .ok_or_else(|| {
                    ApiError::NotFound(format!("produk #{} tidak ditemukan", item.product_id))
                })?;
            let already = lines
                .iter()
                .filter(|l: &&OrderLine| l.product_id == product.id)
                .map(|l| l.quantity)
                .sum::<u32>();
            if already + item.quantity > product.stock {
                return Err(ApiError::Conflict(format!(
                    "stok {} tersisa {}",
                    product.name, product.stock
                )));
            }
            lines.push(OrderLine {
                product_id: product.id,
                name: product.name,
                quantity: item.quantity,
                subtotal: product.price * u64::from(item.quantity),
            });
        }

        for line in &lines {
            if let Some(p) = store.products.iter_mut().find(|p| p.id == line.product_id) {
                p.stock -= line.quantity;
            }
        }

        let order = Order {
            id: format!("NM-{:05}", store.orders.len() + 1),
            customer: customer.to_string(),
            total: lines.iter().map(|l| l.subtotal).sum(),
            items: lines,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
        };
        store.orders.push(order.clone());
        tracing::info!(order = %order.id, customer = %order.customer, total = order.total, "order baru masuk");
        Ok(order)
    }

    /// Kembalikan stok tiap produk ke jumlah awal; mengembalikan jumlah unit yang ditambahkan.
    pub fn restock(&self) -> u32 {
        let seed = catalog::seed();
        let mut store = self.store();
        let mut added = 0;
        for product in &mut store.products {
            if let Some(initial) = seed.iter().find(|p| p.id == product.id) {
                added += initial.stock.saturating_sub(product.stock);
                product.stock = product.stock.max(initial.stock);
            }
        }
        added
    }

    pub fn stats(&self) -> Stats {
        let store = self.store();
        Stats {
            orders: store.orders.len(),
            revenue: store.orders.iter().map(|o| o.total).sum(),
            items_sold: store
                .orders
                .iter()
                .flat_map(|o| &o.items)
                .map(|l| l.quantity)
                .sum(),
            products: store.products.len(),
            stock_units: store.products.iter().map(|p| p.stock).sum(),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

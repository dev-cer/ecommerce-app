//! Lapisan penyimpanan: PostgreSQL untuk produksi (multi-replika), in-memory untuk dev & test cepat.

mod memory;
mod postgres;

use serde::Serialize;

pub use memory::MemoryStore;
pub use postgres::PgStore;

use crate::{catalog::Product, error::ApiResult};

/// Jumlah order terbaru yang dikembalikan `GET /api/orders`.
pub const RECENT_ORDERS: i64 = 50;

#[derive(Clone)]
pub enum Store {
    Memory(MemoryStore),
    Postgres(PgStore),
}

/// Order yang sudah lolos validasi; baris sudah digabung per produk dan diurutkan berdasarkan id
/// (urutan tetap mencegah deadlock antar transaksi di Postgres).
#[derive(Debug)]
pub struct NewOrder {
    pub customer: String,
    pub lines: Vec<(i32, i32)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: String,
    pub customer: String,
    pub items: Vec<OrderLine>,
    pub total: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderLine {
    pub product_id: i32,
    pub name: String,
    pub quantity: i32,
    pub subtotal: i64,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub orders: i64,
    pub revenue: i64,
    pub items_sold: i64,
    pub products: i64,
    pub stock_units: i64,
}

fn order_id(seq: i64) -> String {
    format!("NM-{seq:05}")
}

impl Store {
    pub fn kind(&self) -> &'static str {
        match self {
            Store::Memory(_) => "memory",
            Store::Postgres(_) => "postgres",
        }
    }

    pub async fn products(&self) -> ApiResult<Vec<Product>> {
        match self {
            Store::Memory(s) => Ok(s.products()),
            Store::Postgres(s) => s.products().await,
        }
    }

    pub async fn product(&self, id: i32) -> ApiResult<Option<Product>> {
        match self {
            Store::Memory(s) => Ok(s.product(id)),
            Store::Postgres(s) => s.product(id).await,
        }
    }

    pub async fn recent_orders(&self) -> ApiResult<Vec<Order>> {
        match self {
            Store::Memory(s) => Ok(s.recent_orders()),
            Store::Postgres(s) => s.recent_orders().await,
        }
    }

    pub async fn place_order(&self, order: NewOrder) -> ApiResult<Order> {
        match self {
            Store::Memory(s) => s.place_order(order),
            Store::Postgres(s) => s.place_order(order).await,
        }
    }

    pub async fn restock(&self) -> ApiResult<i64> {
        match self {
            Store::Memory(s) => Ok(s.restock()),
            Store::Postgres(s) => s.restock().await,
        }
    }

    pub async fn stats(&self) -> ApiResult<Stats> {
        match self {
            Store::Memory(s) => Ok(s.stats()),
            Store::Postgres(s) => s.stats().await,
        }
    }

    /// Dipakai readiness probe: pod baru menerima traffic setelah database terjangkau.
    pub async fn ready(&self) -> bool {
        match self {
            Store::Memory(_) => true,
            Store::Postgres(s) => s.ping().await,
        }
    }
}

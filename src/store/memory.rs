use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{NewOrder, Order, OrderLine, RECENT_ORDERS, Stats, order_id};
use crate::{
    catalog::{self, Product},
    error::{ApiError, ApiResult},
};

/// Penyimpanan dalam proses: tanpa dependency, tapi hanya cocok untuk satu replika.
#[derive(Clone)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    products: Vec<Product>,
    orders: Vec<Order>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                products: catalog::seed(),
                orders: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Data tetap konsisten walau ada handler yang panic.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn products(&self) -> Vec<Product> {
        self.lock().products.clone()
    }

    pub fn product(&self, id: i32) -> Option<Product> {
        self.lock().products.iter().find(|p| p.id == id).cloned()
    }

    pub fn recent_orders(&self) -> Vec<Order> {
        let inner = self.lock();
        inner
            .orders
            .iter()
            .rev()
            .take(RECENT_ORDERS as usize)
            .cloned()
            .collect()
    }

    /// Validasi seluruh baris dulu, baru kurangi stok: order bersifat all-or-nothing.
    pub fn place_order(&self, order: NewOrder) -> ApiResult<Order> {
        let mut inner = self.lock();
        let mut items = Vec::with_capacity(order.lines.len());
        for &(product_id, quantity) in &order.lines {
            let product = inner
                .products
                .iter()
                .find(|p| p.id == product_id)
                .ok_or_else(|| {
                    ApiError::NotFound(format!("produk #{product_id} tidak ditemukan"))
                })?;
            if quantity > product.stock {
                return Err(ApiError::Conflict(format!(
                    "stok {} tersisa {}",
                    product.name, product.stock
                )));
            }
            items.push(OrderLine {
                product_id,
                name: product.name.clone(),
                quantity,
                subtotal: product.price * i64::from(quantity),
            });
        }

        for line in &items {
            if let Some(p) = inner.products.iter_mut().find(|p| p.id == line.product_id) {
                p.stock -= line.quantity;
            }
        }

        let order = Order {
            id: order_id(inner.orders.len() as i64 + 1),
            customer: order.customer,
            total: items.iter().map(|l| l.subtotal).sum(),
            items,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default(),
        };
        inner.orders.push(order.clone());
        Ok(order)
    }

    pub fn restock(&self) -> i64 {
        let seed = catalog::seed();
        let mut inner = self.lock();
        let mut added = 0;
        for product in &mut inner.products {
            if let Some(initial) = seed.iter().find(|p| p.id == product.id)
                && product.stock < initial.stock
            {
                added += i64::from(initial.stock - product.stock);
                product.stock = initial.stock;
            }
        }
        added
    }

    pub fn stats(&self) -> Stats {
        let inner = self.lock();
        Stats {
            orders: inner.orders.len() as i64,
            revenue: inner.orders.iter().map(|o| o.total).sum(),
            items_sold: inner
                .orders
                .iter()
                .flat_map(|o| &o.items)
                .map(|l| i64::from(l.quantity))
                .sum(),
            products: inner.products.len() as i64,
            stock_units: inner.products.iter().map(|p| i64::from(p.stock)).sum(),
        }
    }
}

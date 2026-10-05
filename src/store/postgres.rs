use std::time::Duration;

use sqlx::{
    PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use super::{NewOrder, Order, OrderLine, RECENT_ORDERS, Stats, order_id};
use crate::{
    catalog::{self, Product},
    error::{ApiError, ApiResult},
};

#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Koneksi + migrasi + seed katalog. Di Kubernetes database bisa siap belakangan,
    /// jadi koneksi dicoba ulang sebelum menyerah (pod lalu di-restart oleh kubelet).
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        Self::connect_with(url.parse()?, max_connections, 30).await
    }

    pub async fn connect_with(
        options: PgConnectOptions,
        max_connections: u32,
        attempts: u32,
    ) -> Result<Self, sqlx::Error> {
        let mut attempt = 1;
        let pool = loop {
            match PgPoolOptions::new()
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(5))
                .connect_with(options.clone())
                .await
            {
                Ok(pool) => break pool,
                Err(err) if attempt < attempts => {
                    tracing::warn!(attempt, %err, "database belum siap, mencoba lagi…");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        };

        // Migrasi memakai advisory lock, jadi aman walau beberapa replika start bersamaan.
        sqlx::migrate!("./migrations").run(&pool).await?;
        let store = Self { pool };
        store.seed().await?;
        Ok(store)
    }

    /// Masukkan katalog awal; produk yang sudah ada (beserta stoknya) tidak disentuh.
    async fn seed(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for p in catalog::seed() {
            sqlx::query(
                "INSERT INTO products (id, name, category, description, emoji, price, rating, stock, initial_stock)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(p.id)
            .bind(&p.name)
            .bind(&p.category)
            .bind(&p.description)
            .bind(&p.emoji)
            .bind(p.price)
            .bind(p.rating)
            .bind(p.stock)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    pub async fn ping(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }

    pub async fn products(&self) -> ApiResult<Vec<Product>> {
        Ok(sqlx::query_as(
            "SELECT id, name, category, description, emoji, price, rating, stock FROM products ORDER BY id",
        )
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn product(&self, id: i32) -> ApiResult<Option<Product>> {
        Ok(sqlx::query_as(
            "SELECT id, name, category, description, emoji, price, rating, stock FROM products WHERE id = $1",
        )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Satu transaksi: stok dikurangi dengan `UPDATE … WHERE stock >= qty`, jadi dua replika
    /// yang menerima order bersamaan tidak akan pernah menjual melebihi stok.
    pub async fn place_order(&self, order: NewOrder) -> ApiResult<Order> {
        let mut tx = self.pool.begin().await?;
        let mut items = Vec::with_capacity(order.lines.len());

        for &(product_id, quantity) in &order.lines {
            let reserved: Option<(String, i64)> = sqlx::query_as(
                "UPDATE products SET stock = stock - $2 WHERE id = $1 AND stock >= $2 RETURNING name, price",
            )
            .bind(product_id)
            .bind(quantity)
            .fetch_optional(&mut *tx)
            .await?;

            match reserved {
                Some((name, price)) => items.push(OrderLine {
                    product_id,
                    name,
                    quantity,
                    subtotal: price * i64::from(quantity),
                }),
                // Transaksi di-rollback otomatis saat `tx` di-drop.
                None => return Err(Self::rejection(&mut tx, product_id).await?),
            }
        }

        let total: i64 = items.iter().map(|l| l.subtotal).sum();
        let (seq, created_at): (i64, i64) = sqlx::query_as(
            "INSERT INTO orders (customer, total) VALUES ($1, $2)
             RETURNING id, EXTRACT(EPOCH FROM created_at)::BIGINT",
        )
        .bind(&order.customer)
        .bind(total)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO order_items (order_id, product_id, name, quantity, subtotal)
             SELECT $1, * FROM UNNEST($2::INTEGER[], $3::TEXT[], $4::INTEGER[], $5::BIGINT[])",
        )
        .bind(seq)
        .bind(items.iter().map(|l| l.product_id).collect::<Vec<_>>())
        .bind(items.iter().map(|l| l.name.clone()).collect::<Vec<_>>())
        .bind(items.iter().map(|l| l.quantity).collect::<Vec<_>>())
        .bind(items.iter().map(|l| l.subtotal).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(Order {
            id: order_id(seq),
            customer: order.customer,
            items,
            total,
            created_at,
        })
    }

    /// Jelaskan kenapa stok tidak bisa dikurangi: produk tidak ada (404) atau stok kurang (409).
    async fn rejection(tx: &mut Transaction<'_, Postgres>, product_id: i32) -> ApiResult<ApiError> {
        let current: Option<(String, i32)> =
            sqlx::query_as("SELECT name, stock FROM products WHERE id = $1")
                .bind(product_id)
                .fetch_optional(&mut **tx)
                .await?;
        Ok(match current {
            None => ApiError::NotFound(format!("produk #{product_id} tidak ditemukan")),
            Some((name, stock)) => ApiError::Conflict(format!("stok {name} tersisa {stock}")),
        })
    }

    pub async fn recent_orders(&self) -> ApiResult<Vec<Order>> {
        let headers: Vec<(i64, String, i64, i64)> = sqlx::query_as(
            "SELECT id, customer, total, EXTRACT(EPOCH FROM created_at)::BIGINT
             FROM orders ORDER BY id DESC LIMIT $1",
        )
        .bind(RECENT_ORDERS)
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<i64> = headers.iter().map(|h| h.0).collect();
        let lines: Vec<(i64, i32, String, i32, i64)> = sqlx::query_as(
            "SELECT order_id, product_id, name, quantity, subtotal
             FROM order_items WHERE order_id = ANY($1) ORDER BY product_id",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(headers
            .into_iter()
            .map(|(seq, customer, total, created_at)| Order {
                id: order_id(seq),
                customer,
                total,
                created_at,
                items: lines
                    .iter()
                    .filter(|l| l.0 == seq)
                    .map(|(_, product_id, name, quantity, subtotal)| OrderLine {
                        product_id: *product_id,
                        name: name.clone(),
                        quantity: *quantity,
                        subtotal: *subtotal,
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn restock(&self) -> ApiResult<i64> {
        let (added,): (i64,) = sqlx::query_as(
            "WITH refilled AS (
                 UPDATE products p SET stock = p.initial_stock
                 FROM (SELECT id, initial_stock - stock AS added FROM products
                       WHERE stock < initial_stock FOR UPDATE) low
                 WHERE p.id = low.id
                 RETURNING low.added
             )
             SELECT COALESCE(SUM(added), 0)::BIGINT FROM refilled",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(added)
    }

    pub async fn stats(&self) -> ApiResult<Stats> {
        let (orders, revenue, items_sold, products, stock_units): (i64, i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT
                     (SELECT COUNT(*) FROM orders),
                     (SELECT COALESCE(SUM(total), 0)::BIGINT FROM orders),
                     (SELECT COALESCE(SUM(quantity), 0)::BIGINT FROM order_items),
                     (SELECT COUNT(*) FROM products),
                     (SELECT COALESCE(SUM(stock), 0)::BIGINT FROM products)",
            )
            .fetch_one(&self.pool)
            .await?;
        Ok(Stats {
            orders,
            revenue,
            items_sold,
            products,
            stock_units,
        })
    }
}

//! Penyimpanan katalog: PostgreSQL (data di PVC, dipakai di Kubernetes) atau in-memory (lokal & test).

use std::{sync::Arc, time::Duration};

use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls, Row};

use crate::catalog::{self, Product};

const SCHEMA: &str = include_str!("../migrations/0001_init.sql");
const COLUMNS: &str = "id, name, category, description, emoji, price, rating";

#[derive(Clone)]
pub enum Store {
    Memory(Arc<Vec<Product>>),
    Postgres(Arc<PgStore>),
}

pub struct PgStore {
    url: String,
    /// Satu koneksi per pod (tokio-postgres mem-pipeline query), dibuat ulang otomatis bila putus.
    client: Mutex<Option<Arc<Client>>>,
}

impl Store {
    pub fn memory() -> Self {
        Store::Memory(Arc::new(catalog::seed()))
    }

    /// Hubungkan ke PostgreSQL, buat skema & seed. Dicoba ulang karena pod database bisa siap
    /// belakangan (mis. volume PVC baru di-attach).
    pub async fn postgres(url: &str, attempts: u32) -> Result<Self, tokio_postgres::Error> {
        let store = PgStore {
            url: url.to_string(),
            client: Mutex::new(None),
        };
        let mut attempt = 1;
        loop {
            match store.init().await {
                Ok(()) => return Ok(Store::Postgres(Arc::new(store))),
                Err(err) if attempt < attempts => {
                    eprintln!("PostgreSQL belum siap ({attempt}/{attempts}): {err}");
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Store::Memory(_) => "memory",
            Store::Postgres(_) => "postgres",
        }
    }

    /// Filter kategori (persis) dan pencarian teks (nama/deskripsi, tanpa beda huruf besar-kecil).
    pub async fn search(
        &self,
        category: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<Product>, tokio_postgres::Error> {
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        match self {
            Store::Memory(products) => {
                let query = query.map(str::to_lowercase);
                Ok(products
                    .iter()
                    .filter(|p| category.is_none_or(|c| p.category == c))
                    .filter(|p| {
                        query.as_ref().is_none_or(|q| {
                            p.name.to_lowercase().contains(q)
                                || p.description.to_lowercase().contains(q)
                        })
                    })
                    .cloned()
                    .collect())
            }
            Store::Postgres(pg) => {
                let sql = format!(
                    "SELECT {COLUMNS} FROM products \
                     WHERE ($1::text IS NULL OR category = $1) \
                       AND ($2::text IS NULL OR name ILIKE '%' || $2 || '%' OR description ILIKE '%' || $2 || '%') \
                     ORDER BY id"
                );
                let rows = pg.client().await?.query(&sql, &[&category, &query]).await?;
                Ok(rows.iter().map(product).collect())
            }
        }
    }

    pub async fn product(&self, id: i32) -> Result<Option<Product>, tokio_postgres::Error> {
        match self {
            Store::Memory(products) => Ok(products.iter().find(|p| p.id == id).cloned()),
            Store::Postgres(pg) => {
                let sql = format!("SELECT {COLUMNS} FROM products WHERE id = $1");
                let row = pg.client().await?.query_opt(&sql, &[&id]).await?;
                Ok(row.as_ref().map(product))
            }
        }
    }

    /// Readiness: database terjangkau.
    pub async fn ready(&self) -> bool {
        match self {
            Store::Memory(_) => true,
            Store::Postgres(pg) => match pg.client().await {
                Ok(client) => client.simple_query("SELECT 1").await.is_ok(),
                Err(_) => false,
            },
        }
    }
}

impl PgStore {
    async fn client(&self) -> Result<Arc<Client>, tokio_postgres::Error> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref().filter(|c| !c.is_closed()) {
            return Ok(client.clone());
        }
        let (client, connection) = tokio_postgres::connect(&self.url, NoTls).await?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("koneksi PostgreSQL terputus: {err}");
            }
        });
        let client = Arc::new(client);
        *slot = Some(client.clone());
        Ok(client)
    }

    async fn init(&self) -> Result<(), tokio_postgres::Error> {
        let client = self.client().await?;
        client.batch_execute(SCHEMA).await?;
        let count: i64 = client
            .query_one("SELECT count(*) FROM products", &[])
            .await?
            .get(0);
        if count == 0 {
            for p in catalog::seed() {
                client
                    .execute(
                        &format!(
                            "INSERT INTO products ({COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7)"
                        ),
                        &[
                            &p.id,
                            &p.name,
                            &p.category,
                            &p.description,
                            &p.emoji,
                            &p.price,
                            &p.rating,
                        ],
                    )
                    .await?;
            }
            println!(
                "PostgreSQL: katalog di-seed ({} produk)",
                catalog::seed().len()
            );
        }
        Ok(())
    }
}

fn product(row: &Row) -> Product {
    Product {
        id: row.get("id"),
        name: row.get("name"),
        category: row.get("category"),
        description: row.get("description"),
        emoji: row.get("emoji"),
        price: row.get("price"),
        rating: row.get("rating"),
    }
}

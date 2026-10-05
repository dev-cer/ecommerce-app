//! Rekomendasi produk: skor kemiripan dengan "model" yang sengaja berat di CPU.
//!
//! Endpoint ini menjadi beban CPU yang realistis untuk demo autoscaling (HPA lalu Cluster
//! Autoscaler): murni komputasi, tanpa query database, sehingga yang membesar adalah kebutuhan
//! pod/node aplikasi, bukan PostgreSQL. Beratnya diatur lewat env `RECO_WORK`.

use serde::Serialize;

use crate::{
    catalog::{self, Product},
    error::{ApiError, ApiResult},
};

pub const MAX_RESULTS: usize = 4;

#[derive(Debug, Serialize)]
pub struct Recommendation {
    pub product_id: i32,
    pub name: String,
    pub emoji: String,
    pub price: i64,
    pub score: f64,
}

/// Hitung rekomendasi untuk `product_id`. `work` = jumlah putaran "model" per kandidat.
pub fn recommend(product_id: i32, work: u32) -> ApiResult<Vec<Recommendation>> {
    let catalog = catalog::seed();
    let base = catalog
        .iter()
        .find(|p| p.id == product_id)
        .ok_or_else(|| ApiError::NotFound(format!("produk #{product_id} tidak ditemukan")))?;

    let mut scored: Vec<(f64, &Product)> = catalog
        .iter()
        .filter(|p| p.id != base.id)
        .map(|p| (score(base, p, work), p))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    Ok(scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(score, p)| Recommendation {
            product_id: p.id,
            name: p.name.clone(),
            emoji: p.emoji.clone(),
            price: p.price,
            score: (score * 1000.0).round() / 1000.0,
        })
        .collect())
}

/// Kategori sama paling berpengaruh, lalu rating, lalu "afinitas" hasil komputasi berulang.
fn score(base: &Product, candidate: &Product, work: u32) -> f64 {
    let mut h = ((base.id as u64) << 32) | candidate.id as u64 | 1;
    for _ in 0..work {
        // xorshift64: murah per putaran, tapi tidak bisa dihilangkan compiler karena hasilnya dipakai.
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
    }
    let affinity = (std::hint::black_box(h) % 1000) as f64 / 1000.0;
    let same_category = if candidate.category == base.category {
        2.0
    } else {
        0.0
    };
    same_category + f64::from(candidate.rating) / 5.0 + affinity * 0.5
}

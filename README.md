# 🛒 NusaMart — E-commerce Demo

Storefront + REST API dalam **satu binary Rust** (Axum + Tokio). Tanpa database, tanpa setup — cocok untuk demo sprint review / showcase agile.

## Jalankan

```bash
cargo run                 # http://localhost:8080
PORT=3000 cargo run       # port lain
```

Atau dengan Docker:

```bash
docker build -t nusamart .
docker run --rm -p 8080:8080 nusamart
```

## Alur demo (± 3 menit)

1. Buka `http://localhost:8080` — tunjukkan katalog, filter kategori, dan pencarian.
2. Tambahkan beberapa produk ke keranjang, atur jumlahnya, lalu checkout.
3. Perhatikan statistik di banner (order, item terjual, omzet) dan stok produk langsung berubah.
4. Tunjukkan API-nya langsung:

```bash
curl localhost:8080/api/products?category=makanan
curl localhost:8080/api/stats
curl -X POST localhost:8080/api/orders \
  -H 'content-type: application/json' \
  -d '{"customer":"Budi","items":[{"product_id":1,"quantity":2}]}'
```

5. Coba pesan melebihi stok → API menolak dengan `409 Conflict`.

## API

| Method | Endpoint              | Keterangan                                   |
|--------|-----------------------|----------------------------------------------|
| GET    | `/`                   | Storefront web                               |
| GET    | `/health`             | Status, versi, uptime                        |
| GET    | `/api/products`       | Daftar produk (`?category=` & `?q=` opsional) |
| GET    | `/api/products/{id}`  | Detail produk                                |
| GET    | `/api/orders`         | Daftar order (terbaru dulu)                  |
| POST   | `/api/orders`         | Buat order — validasi stok, all-or-nothing   |
| GET    | `/api/stats`          | Ringkasan order, item terjual, omzet         |

## Struktur

```
src/
├── main.rs      # bootstrap server, logging, graceful shutdown
├── routes.rs    # definisi route & handler HTTP
├── state.rs     # state in-memory: katalog, order, statistik
├── catalog.rs   # data produk awal
└── error.rs     # error API → JSON
static/
└── index.html   # storefront (di-embed ke binary saat compile)
```

Data disimpan di memori, jadi restart server = reset demo.

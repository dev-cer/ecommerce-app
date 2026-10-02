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
| GET    | `/metrics`            | Metrik Prometheus (bisnis, request HTTP, histogram latensi) |

## CI/CD & GitOps (KCD Bandung 2026)

```
git push ─▶ Jenkins ─────────────────────────────────────────────────▶ GitHub (k8s/) ─▶ ArgoCD ─▶ Civo K8s
            ├─ Lint + unit & security test (cargo)  ┐ paralel             commit tag          sync     rolling update
            ├─ SCA & secret scan (Trivy fs)         ┘                     [skip ci]
            ├─ Server sementara di pod ─┬─ DAST (OWASP ZAP baseline)
            │                           └─ Load test (k6, profil smoke + threshold)
            ├─ Build & push image (Kaniko → ghcr.io)
            ├─ Image scan (Trivy, gagal jika ada CRITICAL)
            └─ GitOps: update manifest
```

- `Jenkinsfile` — pipeline di atas. Build image memakai **Kaniko** karena agent Jenkins berjalan di dalam Kubernetes tanpa Docker daemon.
- `k8s/` — manifest yang disinkronkan ArgoCD: Deployment, Service `ecommerce-rust-svc`, ServiceMonitor, dan dashboard Grafana.
- `deploy/master-demo.ps1` — provisioning end-to-end: Terraform → ArgoCD → Jenkins (job dibuat otomatis via JCasC) → Prometheus/Grafana.

```powershell
$env:GITHUB_USER  = "dev-cer"
$env:GITHUB_TOKEN = "<PAT: repo, write:packages>"   # jangan pernah ditulis di skrip/repo
./deploy/master-demo.ps1 -TerraformDir ../infra
```

Footer storefront menampilkan `build <tag> · pod <nama>`, jadi audience bisa melihat versi baru muncul setelah ArgoCD selesai rollout.

## Security testing

| Lapisan | Alat | Yang dicek |
|---|---|---|
| Unit/integration test | `cargo test` (`src/tests.rs`) | Header keamanan, validasi input, overflow jumlah, payload raksasa (413), content-type salah (415), fuzzing path/query, order all-or-nothing, **50 order paralel tidak pernah oversell** |
| SCA & secret | Trivy `fs` | CVE di `Cargo.lock`, token/secret yang ter-commit |
| DAST | OWASP ZAP baseline | Scan aplikasi yang berjalan; aturan di `.zap/rules.tsv` (CSP, anti-clickjacking, nosniff = FAIL) |
| Container | Trivy `image` | CVE CRITICAL di image final |

Hardening di aplikasi: Content-Security-Policy ketat (tanpa inline script/style), `X-Frame-Options`, `nosniff`, `Permissions-Policy`, batas body 16 KB, timeout request 10 detik, batas panjang nama, jumlah item, dan baris order. Container berjalan non-root dengan root filesystem read-only.

## Stress test (k6)

```bash
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=smoke  tests/load/stress.js   # 30 detik, dipakai di CI
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=stress tests/load/stress.js   # naik bertahap ke 300 VU
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=spike  tests/load/stress.js   # lonjakan 400 VU
```

Di klaster (tanpa perlu k6 di laptop): `./deploy/stress-test.ps1 -Profile stress`. Script k6 dijalankan sebagai Job di system-pool, sementara dashboard Grafana menampilkan RPS, latensi p50/p95/p99, traffic per status, stok, dan CPU.

Threshold: error < 1%, p95 baca < 200 ms (smoke) / 500 ms (stress), p95 tulis < 300 / 800 ms. Respons `409` (stok habis) dihitung sebagai perilaku benar, bukan error. Stok otomatis di-restock tiap 60 detik (`RESTOCK_INTERVAL_SECS`, `0` = mati), jadi storefront tetap bisa dipakai setelah stress test.

## Struktur

```
src/
├── main.rs      # bootstrap server, logging, graceful shutdown
├── routes.rs    # definisi route & handler HTTP
├── state.rs     # state in-memory: katalog, order, statistik
├── catalog.rs   # data produk awal
├── error.rs     # error API → JSON
├── metrics.rs   # metrik Prometheus + middleware latensi
├── security.rs  # header keamanan (CSP dkk.)
└── tests.rs     # test fungsional, keamanan & konkurensi
static/          # storefront: index.html, app.css, app.js (di-embed ke binary)
tests/load/      # skenario k6
.zap/            # aturan OWASP ZAP
k8s/             # manifest GitOps (ArgoCD)
deploy/          # master-demo.ps1 (provisioning) & stress-test.ps1
Jenkinsfile      # pipeline CI/DevSecOps
```

Data disimpan di memori, jadi restart server = reset demo.

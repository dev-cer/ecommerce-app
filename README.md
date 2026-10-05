# 🛒 NusaMart — E-commerce Demo

Storefront + REST API dalam **satu binary Rust** (Axum + Tokio) dengan **PostgreSQL**. Siap multi-replika dan autoscaling di Kubernetes, tapi tetap bisa jalan tanpa database untuk demo cepat.

## Jalankan

```bash
docker compose up --build   # app + PostgreSQL → http://localhost:8080
cargo run                   # tanpa database (penyimpanan in-memory, 1 replika saja)
```

| Env | Default | Keterangan |
|---|---|---|
| `DATABASE_URL` | – | Jika di-set, pakai PostgreSQL (migrasi & seed katalog otomatis saat start) |
| `DB_MAX_CONNECTIONS` | `10` | Ukuran pool koneksi per pod |
| `PORT` | `8080` | Port HTTP |
| `RESTOCK_INTERVAL_SECS` | `60` | Interval restock otomatis, `0` = mati |
| `APP_VERSION` | `dev` | Tag build (diisi CI), tampil di footer |

## Penyimpanan & skalabilitas

- **PostgreSQL** (`src/store/postgres.rs`) menyimpan produk, order, dan item order. Skema ada di `migrations/`.
- Order diproses dalam **satu transaksi**: stok dikurangi dengan `UPDATE … WHERE stock >= qty`, sehingga banyak pod yang melayani order bersamaan **tidak pernah menjual melebihi stok**. Constraint `CHECK (stock >= 0)` menjadi pengaman terakhir. Baris diproses berurutan per id produk untuk mencegah deadlock.
- Karena state tidak lagi di memori pod, aplikasi berjalan **2–6 replika dengan HPA** (`k8s/hpa.yaml`, target CPU 40%).
- `/health` = liveness (proses hidup); `/ready` = readiness (database terjangkau). Pod tanpa koneksi DB otomatis dikeluarkan dari load balancer.
- Tanpa `DATABASE_URL`, aplikasi memakai store in-memory (`src/store/memory.rs`) dengan perilaku yang sama. Ini praktis untuk dev, tapi hanya untuk 1 replika.

Test berjalan untuk kedua backend:

```bash
cargo test                                                                  # in-memory
TEST_DATABASE_URL=postgres://postgres@localhost:5432/postgres cargo test    # + PostgreSQL
```

Dengan PostgreSQL, tiap test membuat database sementara sendiri, termasuk test **dua replika** yang memproses 60 order paralel untuk stok 6.

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
| GET    | `/health`             | Liveness: status, versi, build, pod, storage |
| GET    | `/ready`              | Readiness: 200 jika database terjangkau      |
| GET    | `/api/products`       | Daftar produk (`?category=` & `?q=` opsional) |
| GET    | `/api/products/{id}`  | Detail produk                                |
| GET    | `/api/orders`         | Daftar order (terbaru dulu)                  |
| POST   | `/api/orders`         | Buat order — validasi stok, all-or-nothing   |
| GET    | `/api/stats`          | Ringkasan order, item terjual, omzet         |
| GET    | `/api/recommendations?product_id=` | Rekomendasi produk (CPU-berat ±3 ms, beban demo autoscaling) |
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
- `k8s/` — manifest yang disinkronkan ArgoCD: Deployment, Service `ecommerce-rust-svc`, ServiceMonitor, dan dua dashboard Grafana: **NusaMart · Live Demo** dan **Civo Quota & Kapasitas Klaster** (gauge ala halaman Quota Civo; dibuat dari `deploy/dashboards/civo_quota.py`, ubah kuota akun di variabel `QUOTA` lalu jalankan ulang skripnya).
- `deploy/master-demo.ps1` — provisioning end-to-end: Terraform → ArgoCD → Jenkins (job dibuat otomatis via JCasC) → Prometheus/Grafana.

```powershell
$env:GITHUB_USER  = "dev-cer"
$env:GITHUB_TOKEN = "<PAT: repo, write:packages>"   # jangan pernah ditulis di skrip/repo
./deploy/master-demo.ps1        # Terraform di infra/ (atau -TerraformDir <folder lain>)
```

Footer storefront menampilkan `build <tag> · pod <nama>`, jadi audience bisa melihat versi baru muncul setelah ArgoCD selesai rollout, dan nama pod berganti-ganti karena load balancing antar replika.

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
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=spike  tests/load/stress.js   # lonjakan 400 VU ditahan 2 menit
```

Di klaster (tanpa perlu k6 di laptop): `./deploy/stress-test.ps1 -Profile stress`. Script k6 dijalankan sebagai Job di system-pool, sementara dashboard Grafana menampilkan RPS, latensi p50/p95/p99, traffic per status, stok, CPU, dan **jumlah replika HPA yang naik-turun** (`kubectl get hpa ecommerce-rust -w`).

**Autoscaling sampai node** (`-Profile scale`, ±8 menit): 600 user + endpoint rekomendasi yang berat di CPU. HPA naik sampai 25 pod; karena tidak muat di 2 node `app-pool`, sebagian pod `Pending`, lalu **Cluster Autoscaler** (ACT 4 di `master-demo.ps1`, butuh API key Civo di `$env:CIVO_TOKEN` atau konfigurasi `civo` CLI) menambah node `app-pool` (maks. 4). Setelah beban turun, pod dan node dilepas lagi. Pantau dengan `kubectl get nodes -L kubernetes.civo.com/civo-node-pool -w`.

Threshold: error < 1%, p95 baca < 200 ms (smoke) / 500 ms (stress), p95 tulis < 300 / 800 ms. Respons `409` (stok habis) dihitung sebagai perilaku benar, bukan error. Stok otomatis di-restock tiap 60 detik (`RESTOCK_INTERVAL_SECS`, `0` = mati), jadi storefront tetap bisa dipakai setelah stress test.

## Struktur

```
src/
├── main.rs      # bootstrap server, logging, graceful shutdown
├── routes.rs    # definisi route & handler HTTP
├── state.rs     # AppState + validasi order
├── store/       # penyimpanan: mod.rs (antarmuka), postgres.rs, memory.rs
├── catalog.rs   # data produk awal
├── error.rs     # error API → JSON
├── metrics.rs   # metrik Prometheus + middleware latensi
├── security.rs  # header keamanan (CSP dkk.)
└── tests.rs     # test fungsional, keamanan & konkurensi
static/          # storefront: index.html, app.css, app.js (di-embed ke binary)
tests/load/      # skenario k6
.zap/            # aturan OWASP ZAP
migrations/      # skema PostgreSQL (sqlx migrate)
k8s/             # manifest GitOps: app, PostgreSQL StatefulSet, HPA, NetworkPolicy, monitoring
deploy/          # master-demo.ps1 (provisioning) & stress-test.ps1
Jenkinsfile      # pipeline CI/DevSecOps
```

Di Kubernetes, password database disimpan di Secret `postgres-credentials` yang dibuat sekali oleh `deploy/master-demo.ps1` (password acak, tidak pernah masuk Git). NetworkPolicy membatasi akses ke PostgreSQL hanya dari pod aplikasi.

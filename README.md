# 🛒 NusaMart — E-commerce Demo

Katalog produk sampel + REST API dalam **satu binary Rust** (Axum + Tokio) dengan **PostgreSQL** di atas PVC (Civo Volume). Sengaja ringkas: dependency minimal (`tokio-postgres`, tanpa ORM/macro) sehingga build dingin di CI hanya ±1–2 menit.

## Jalankan

```bash
docker compose up --build   # app + PostgreSQL → http://localhost:8080
cargo run                   # tanpa database (katalog in-memory)
```

| Env | Default | Keterangan |
|---|---|---|
| `DATABASE_URL` | – | Jika di-set, pakai PostgreSQL (skema & seed katalog otomatis saat start) |
| `PORT` | `8080` | Port HTTP |
| `RECO_WORK` | `100000` | Berat komputasi rekomendasi (beban CPU untuk demo autoscaling) |
| `APP_VERSION` | `dev` | Tag build (diisi CI), tampil di footer |

## Penyimpanan & skalabilitas

- **PostgreSQL** (`src/store.rs`, StatefulSet di `k8s/postgres.yaml`) menyimpan katalog di **PVC 5 Gi (Civo Volume)**: data tetap ada walau pod database di-restart atau pindah node. Skema `migrations/0001_init.sql` idempotent; katalog di-seed hanya bila tabel kosong.
- Satu koneksi per pod (tokio-postgres mem-pipeline query) yang dibuat ulang otomatis bila putus.
- Aplikasi stateless, sehingga berjalan **2–25 replika dengan HPA** (`k8s/hpa.yaml`, target CPU 30%).
- `/health` = liveness (proses hidup); `/ready` = readiness (database terjangkau). Pod tanpa koneksi DB otomatis dikeluarkan dari load balancer.
- Tanpa `DATABASE_URL`, katalog dibaca dari memori dengan perilaku yang sama (praktis untuk dev).

```bash
cargo test                                                                  # in-memory
TEST_DATABASE_URL=postgres://postgres@localhost:5432/postgres cargo test    # + PostgreSQL
```

## Alur demo (± 3 menit)

1. Buka storefront — tunjukkan katalog, filter kategori, dan pencarian (query ke PostgreSQL).
2. Klik **Lihat mirip** → rekomendasi dihitung di server (beban CPU yang dipakai demo autoscaling).
3. Footer & kotak "Pod" menampilkan build dan pod yang melayani; refresh → pod berganti (load balancing).
4. Grafana: panel **Data PostgreSQL (PVC)** menunjukkan pemakaian volume database.

```bash
curl localhost:8080/api/products?category=Makanan
curl "localhost:8080/api/products?q=kopi"
curl "localhost:8080/api/recommendations?product_id=1"
```

## API

| Method | Endpoint              | Keterangan                                   |
|--------|-----------------------|----------------------------------------------|
| GET    | `/`                   | Storefront web                               |
| GET    | `/health`             | Liveness: status, versi, build, pod, storage |
| GET    | `/ready`              | Readiness: 200 jika database terjangkau      |
| GET    | `/api/products`       | Daftar produk (`?category=` & `?q=` opsional) |
| GET    | `/api/products/{id}`  | Detail produk                                |
| GET    | `/api/recommendations?product_id=` | Rekomendasi produk (CPU-berat ±3 ms, beban demo autoscaling) |
| GET    | `/metrics`            | Metrik Prometheus (build, request HTTP, histogram latensi) |

## CI/CD & GitOps (KCD Bandung 2026)

```
git push ─▶ Jenkins (target < 10 menit, cache dingin) ─────────────────────▶ GitHub (k8s/) ─▶ ArgoCD ─▶ Civo K8s
            ├─ Build & Test: fmt + test (memory & PostgreSQL) + binary release ┐ paralel   commit tag   sync   rolling update
            ├─ SCA & secret scan (Trivy fs)                                    ┘           [skip ci]
            ├─ DAST (ZAP baseline) ┐
            ├─ Load test (k6 smoke) ├ paralel, terhadap server sementara + PostgreSQL sidecar
            ├─ Image (Kaniko) ─▶ Trivy image (tarball) ┘
            └─ GitOps: update manifest (hanya jika semua gate lolos)
```

- `Jenkinsfile` — pipeline di atas. Build image memakai **Kaniko** karena agent Jenkins berjalan di dalam Kubernetes tanpa Docker daemon.
- **Cepat**: binary Rust dikompilasi **sekali** (test & build memakai profil release); Kaniko hanya mengemasnya lewat `Dockerfile.ci` (build lokal tetap memakai `Dockerfile`). Crate, folder `target`, dan database Trivy di-cache di node app-pool (`/var/cache/jenkins-ci`), dan `k8s/ci-prepull.yaml` menarik image CI lebih dulu di tiap node app-pool, sehingga pod build langsung jalan. Build pertama di node baru tetap lebih lama (mengisi cache).
- **Tahan jaringan lambat**: unduhan crates.io tanpa multiplexing HTTP/2 dengan retry (`CARGO_NET_RETRY`, `cargo fetch` diulang 3×); push manifest di-rebase bila `main` bergerak selama build.
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
| Unit/integration test | `cargo test` (`src/tests.rs`) | Header keamanan, method tulis ditolak (405), fuzzing path/query, input SQL injection tetap jadi teks pencarian (query berparameter), integrasi PostgreSQL |
| SCA & secret | Trivy `fs` | CVE di `Cargo.lock`, token/secret yang ter-commit |
| DAST | OWASP ZAP baseline | Scan aplikasi yang berjalan; aturan di `.zap/rules.tsv` (CSP, anti-clickjacking, nosniff = FAIL) |
| Container | Trivy `image` | CVE CRITICAL di image final |

Hardening di aplikasi: Content-Security-Policy ketat (tanpa inline script/style), `X-Frame-Options`, `nosniff`, `Permissions-Policy`; API hanya baca (GET) dengan query berparameter. Container berjalan non-root dengan root filesystem read-only.

## Stress test (k6)

```bash
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=smoke  tests/load/stress.js   # 30 detik, dipakai di CI
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=stress tests/load/stress.js   # naik bertahap ke 300 VU
k6 run -e BASE_URL=http://localhost:8080 -e PROFILE=spike  tests/load/stress.js   # lonjakan 400 VU ditahan 2 menit
```

Di klaster (tanpa perlu k6 di laptop): `./deploy/stress-test.ps1 -Profile stress`. Script k6 dijalankan sebagai Job di system-pool, sementara dashboard Grafana menampilkan RPS, latensi p50/p95/p99, traffic per status, pemakaian PVC PostgreSQL, CPU, dan **jumlah replika HPA yang naik-turun** (`kubectl get hpa ecommerce-rust -w`).

**Autoscaling sampai node** (`-Profile scale`, ±8 menit): 600 user + endpoint rekomendasi yang berat di CPU. HPA naik sampai 25 pod; karena tidak muat di 2 node `app-pool`, sebagian pod `Pending`, lalu **Cluster Autoscaler** (ACT 4 di `master-demo.ps1`, butuh API key Civo di `$env:CIVO_TOKEN` atau konfigurasi `civo` CLI) menambah node `app-pool` (maks. 4). Setelah beban turun, pod dan node dilepas lagi. Pantau di terminal kedua dengan `./deploy/watch-scale.ps1` (CPU vs target, replika HPA, pod `Pending`, node `app-pool`, event `TriggeredScaleUp`). Request CPU pod 250m membuat 2 node `app-pool` hanya muat ±12 pod, jadi beban ±0,9 core saja sudah cukup memicu node baru.

Threshold: error < 1%, p95 baca < 200 ms (smoke) / 500 ms (stress).

## Bongkar demo (bersih sampai network, tanpa langkah manual)

```powershell
$env:CIVO_TOKEN = "<api key Civo>"
.\deploy\destroy-demo.ps1          # minta ketik nama klaster sebagai konfirmasi
```

LoadBalancer (+ IP publik) dan volume PVC dibuat Kubernetes, bukan Terraform; kalau tertinggal, tetap ditagih dan `civo_network` tersangkut "Still destroying...". Pembersihannya kini **otomatis berdasarkan ID**:

- **`terraform destroy` langsung pun bersih.** `infra/main.tf` punya provisioner destroy (`terraform_data.cleanup_*`) yang menjalankan `deploy/cleanup-civo.ps1`. Urutannya: hapus Service LoadBalancer & PVC dari dalam klaster → node pool → klaster → sapu LB/volume/firewall sisa di network → firewall → network. Di macOS/Linux tambahkan `-var powershell=pwsh`.
- **`destroy-demo.ps1`** membaca ID klaster, network & firewall dari Terraform state (tetap ada walau klaster sudah terhapus), membersihkan LB/volume, menjalankan `terraform destroy` (diulang sekali setelah menyapu bila gagal), menyapu sisa terakhir, menghapus konteks kubeconfig lokal, lalu melaporkan apa pun yang tersisa (aman dijalankan ulang).

Pencocokan memakai **ID persis**, bukan nama. Network yang dihapus hanya network buatan `infra/main.tf`: UUID-nya diambil dari Terraform state dan labelnya harus `ecommerce-vpc-kcd-v2`. Network `default` akun (dan LB/volume/firewall di dalamnya) tidak pernah disentuh. kubectl memakai kubeconfig sementara khusus klaster demo.

## Struktur

```
src/
├── main.rs      # bootstrap server, koneksi database, graceful shutdown
├── routes.rs    # definisi route & handler HTTP
├── state.rs     # AppState (store, build, pod, metrik)
├── store.rs     # penyimpanan: PostgreSQL (tokio-postgres) atau in-memory
├── catalog.rs   # data produk awal
├── error.rs     # error API → JSON
├── metrics.rs   # metrik Prometheus + middleware latensi
├── security.rs  # header keamanan (CSP dkk.)
├── recommend.rs # rekomendasi (beban CPU demo autoscaling)
└── tests.rs     # test fungsional, keamanan & PostgreSQL
static/          # storefront: index.html, app.css, app.js (di-embed ke binary)
tests/load/      # skenario k6
.zap/            # aturan OWASP ZAP
migrations/      # skema PostgreSQL (idempotent, dijalankan saat startup)
k8s/             # manifest GitOps: app, PostgreSQL StatefulSet, HPA, NetworkPolicy, monitoring
deploy/          # master-demo.ps1, stress-test.ps1, watch-scale.ps1, destroy-demo.ps1, cleanup-civo.ps1
Jenkinsfile      # pipeline CI/DevSecOps
```

Di Kubernetes, password database disimpan di Secret `postgres-credentials` yang dibuat sekali oleh `deploy/master-demo.ps1` (password acak, tidak pernah masuk Git). NetworkPolicy membatasi akses ke PostgreSQL hanya dari pod aplikasi.

Update untuk simulasi KCD Bandung 2026

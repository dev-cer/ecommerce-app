# ---------- Build stage ----------
FROM rust:1.98-slim-trixie AS builder
WORKDIR /app

# Cache dependency: build dulu dengan main.rs kosong agar layer ini
# hanya dibangun ulang ketika Cargo.toml / Cargo.lock berubah.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/app_repo*

COPY src ./src
COPY static ./static
RUN cargo build --release --locked

# ---------- Runtime stage ----------
FROM debian:trixie-slim
WORKDIR /app
RUN useradd --system --uid 10001 app
COPY --from=builder /app/target/release/app-repo /app/ecommerce-service
USER app

# Diisi CI (Jenkins) dengan tag image, tampil di /health dan footer storefront.
ARG APP_VERSION=dev
ENV APP_VERSION=$APP_VERSION PORT=8080 RUST_LOG=info
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=3s --start-period=5s \
    CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/$PORT && printf 'GET /health HTTP/1.0\\r\\n\\r\\n' >&3 && grep -q ok <&3"]
CMD ["./ecommerce-service"]

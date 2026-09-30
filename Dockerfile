FROM rust:1.98-slim AS builder
WORKDIR /app
COPY . .
# Build aplikasi Rust
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
# Ambil hasil build dari stage sebelumnya (nama binary default sama dengan nama folder: app-repo)
COPY --from=builder /app/target/release/app-repo /app/ecommerce-service
EXPOSE 8080
CMD ["./ecommerce-service"]

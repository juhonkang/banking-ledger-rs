# Multi-stage build for minimal banking ledger image
FROM rust:1.89-slim-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src
COPY src/ src/
RUN touch src/main.rs && cargo build --release

# Runtime — distroless-like minimal
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/banking-ledger /usr/local/bin/banking-ledger
EXPOSE 3001
HEALTHCHECK --interval=30s --timeout=3s CMD curl -f http://localhost:3001/health || exit 1
ENTRYPOINT ["banking-ledger"]

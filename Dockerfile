# syntax=docker/dockerfile:1

# ---- Étape 1 : compilation ----
FROM rust:1-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# Cache des dépendances : on compile d'abord avec un main vide
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs \
    && cargo build --release && rm -rf src

COPY src ./src
COPY templates ./templates
COPY static ./static
COPY migrations ./migrations
RUN touch src/main.rs src/lib.rs && cargo build --release

# ---- Étape 2 : image d'exécution minimale ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /app santa \
    && mkdir -p /data && chown santa /data
WORKDIR /app
COPY --from=builder /app/target/release/secret-santa /usr/local/bin/secret-santa

ENV DATABASE_PATH=/data/santa.db \
    BIND_ADDR=0.0.0.0:3000 \
    RUST_LOG=secret_santa=info

USER santa
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:3000/health || exit 1
CMD ["secret-santa"]

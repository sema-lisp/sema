FROM rust:1.88-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY templates/ templates/
RUN cargo build --release

FROM debian:bookworm-slim
# sqlite3 lets `make dev-docker` seed the admin user from inside the container
# (see seed.sh — the first admin cannot be created through the API).
RUN apt-get update && apt-get install -y ca-certificates sqlite3 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/target/release/sema-pkg /usr/local/bin/
COPY templates/ templates/
COPY static/ static/
EXPOSE 3000
VOLUME ["/app/data"]
CMD ["sema-pkg"]

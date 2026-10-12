# GritIRT backend image: GritShield (crates.io) incident-response service + SQLite.
# Build context is this repo (GritIRT/backend).
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/gritirt-backend /usr/local/bin/gritirt-backend
ENV IRT_BIND=0.0.0.0 \
    IRT_PORT=8030 \
    IRT_DATABASE_URL="sqlite:///data/grit_irt.db?mode=rwc"
EXPOSE 8030
VOLUME /data
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD curl -fsS http://localhost:8030/health || exit 1
CMD ["gritirt-backend"]

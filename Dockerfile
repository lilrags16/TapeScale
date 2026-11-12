# syntax=docker/dockerfile:1.7
FROM rust:1.89.0-bookworm AS builder
WORKDIR /build

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim AS runtime
ARG TAPESCALE_VERSION=0.1.0
LABEL org.opencontainers.image.title="TapeScale" \
      org.opencontainers.image.description="Safety-first LTO backup control plane for TrueNAS" \
      org.opencontainers.image.version="${TAPESCALE_VERSION}" \
      org.opencontainers.image.source="https://github.com/lilrags16/TapeScale" \
      org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl mt-st tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 tapescale \
    && useradd --system --uid 10001 --gid tapescale --home-dir /var/lib/tapescale tapescale \
    && install -d -o tapescale -g tapescale /var/lib/tapescale /var/lib/tapescale/recovery /var/lib/tapescale/spool

COPY --from=builder /build/target/release/tapescale-api /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-scheduler /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-worker /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-recovery /usr/local/bin/
COPY web /app/web
COPY docs /app/docs

USER 10001:10001
WORKDIR /app
ENV TAPESCALE_BIND=0.0.0.0:8080 \
    TAPESCALE_WEB_DIR=/app/web \
    RUST_LOG=tapescale=info,tower_http=info
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "http://127.0.0.1:8080/health/ready"]
ENTRYPOINT ["/usr/local/bin/tapescale-api"]

# syntax=docker/dockerfile:1.7
FROM rust:1.89.0-bookworm@sha256:948f9b08a66e7fe01b03a98ef1c7568292e07ec2e4fe90d88c07bb14563c84ff AS builder
WORKDIR /build

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS runtime
ARG TAPESCALE_VERSION=0.1.0-alpha.7
ARG TAPESCALE_REVISION=unknown
ARG DEBIAN_SNAPSHOT=20250730T000000Z
LABEL org.opencontainers.image.title="TapeScale" \
      org.opencontainers.image.description="Safety-first LTO backup control plane for TrueNAS" \
      org.opencontainers.image.version="${TAPESCALE_VERSION}" \
      org.opencontainers.image.revision="${TAPESCALE_REVISION}" \
      org.opencontainers.image.source="https://github.com/lilrags16/TapeScale" \
      org.opencontainers.image.licenses="Apache-2.0 AND AGPL-3.0-only"

# The pinned bacula-director postinst unconditionally sources
# /usr/share/bacula-common/dbc_database.conf, while dbconfig-no-thanks
# intentionally does not create it. TapeScale initializes the catalog itself.
RUN echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT}/ bookworm-backports main" \
      > /etc/apt/sources.list.d/tapescale-bookworm-backports.list \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
         ca-certificates \
         curl \
         dbconfig-no-thanks \
         mt-st \
         tzdata \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
         -t bookworm-backports \
         bacula-bscan=15.0.3-3~bpo12+1 \
         bacula-common-pgsql=15.0.3-3~bpo12+1 \
         bacula-console=15.0.3-3~bpo12+1 \
         bacula-director-pgsql=15.0.3-3~bpo12+1 \
         bacula-fd=15.0.3-3~bpo12+1 \
         bacula-sd=15.0.3-3~bpo12+1 \
    && install -d -m 0755 /usr/share/bacula-common \
    && printf '%s\n' 'dbc_install=false' > /usr/share/bacula-common/dbc_database.conf \
    && chmod 0644 /usr/share/bacula-common/dbc_database.conf \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
         -t bookworm-backports \
         bacula-director=15.0.3-3~bpo12+1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 tapescale \
    && useradd --system --uid 10001 --gid tapescale --home-dir /var/lib/tapescale tapescale \
    && install -d -m 0770 -o tapescale -g root \
         /var/lib/tapescale \
         /var/lib/tapescale/bacula \
         /var/lib/tapescale/bacula/qualifications \
         /var/lib/tapescale/bacula/run \
         /var/lib/tapescale/bacula/working \
         /var/lib/tapescale/recovery \
         /var/lib/tapescale/spool

COPY --from=builder /build/target/release/tapescale-api /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-scheduler /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-worker /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-recovery /usr/local/bin/
COPY web /app/web
COPY docs /app/docs
COPY LICENSE THIRD_PARTY_NOTICES.md /app/

USER 10001:10001
WORKDIR /app
ENV TAPESCALE_BIND=0.0.0.0:8080 \
    TAPESCALE_WEB_DIR=/app/web \
    RUST_LOG=tapescale=info,tower_http=info
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "http://127.0.0.1:8080/health/ready"]
ENTRYPOINT ["/usr/local/bin/tapescale-api"]

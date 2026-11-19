# syntax=docker/dockerfile:1.7
FROM rust:1.89.0-bookworm@sha256:948f9b08a66e7fe01b03a98ef1c7568292e07ec2e4fe90d88c07bb14563c84ff AS builder
WORKDIR /build

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --release --bins

FROM debian:bookworm-20250721-slim@sha256:2424c1850714a4d94666ec928e24d86de958646737b1d113f5b2207be44d37d8 AS dar-builder
ARG DEBIAN_SNAPSHOT=20250730T000000Z
ENV DAR_VERSION=2.8.6 \
    DAR_SHA256=d56caeb6c86f751cb454d8fef45f3daecc508f208290d9f51e8c75bba5dc46c5 \
    LIBTHREADAR_VERSION=1.6.1 \
    LIBTHREADAR_SHA256=467709320c54215ac8829ef48e856d890caa4f6a7831633d3992e0b5359f38d8 \
    SOURCE_DATE_EPOCH=1787097600

# DAR 2.8.6 is newer than the DAR release in Debian bookworm. Build the
# unmodified, hash-pinned upstream releases against a fixed Debian snapshot.
# Keeping DAR as a separate executable avoids linking GPL code into TapeScale.
RUN printf '%s\n' \
      "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT}/ bookworm main" \
      "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT}/ bookworm-security main" \
      > /etc/apt/sources.list.d/tapescale-dar-builder.list \
    && apt-get \
         -o Acquire::Check-Valid-Until=false \
         -o Dir::Etc::sourcelist=/etc/apt/sources.list.d/tapescale-dar-builder.list \
         -o Dir::Etc::sourceparts=- \
         update \
    && DEBIAN_FRONTEND=noninteractive apt-get \
         -o Dir::Etc::sourcelist=/etc/apt/sources.list.d/tapescale-dar-builder.list \
         -o Dir::Etc::sourceparts=- \
         install --yes --no-install-recommends \
         build-essential \
         ca-certificates \
         curl \
         liblz4-dev \
         libzstd-dev \
         pkg-config \
         zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
RUN curl --fail --location --proto '=https' --tlsv1.2 --retry 5 --retry-all-errors \
      "https://downloads.sourceforge.net/project/libthreadar/${LIBTHREADAR_VERSION}/libthreadar-${LIBTHREADAR_VERSION}.tar.gz" \
      --output "libthreadar-${LIBTHREADAR_VERSION}.tar.gz" \
    && echo "${LIBTHREADAR_SHA256}  libthreadar-${LIBTHREADAR_VERSION}.tar.gz" | sha256sum --check --strict \
    && tar --extract --gzip --file "libthreadar-${LIBTHREADAR_VERSION}.tar.gz" \
    && cd "libthreadar-${LIBTHREADAR_VERSION}" \
    && ./configure \
         --prefix=/usr/local \
         --disable-static \
         --enable-shared \
         --disable-dependency-tracking \
    && make -j2 \
    && make install-strip

RUN curl --fail --location --proto '=https' --tlsv1.2 --retry 5 --retry-all-errors \
      "https://downloads.sourceforge.net/project/dar/dar/${DAR_VERSION}/dar-${DAR_VERSION}.tar.gz" \
      --output "dar-${DAR_VERSION}.tar.gz" \
    && echo "${DAR_SHA256}  dar-${DAR_VERSION}.tar.gz" | sha256sum --check --strict \
    && tar --extract --gzip --file "dar-${DAR_VERSION}.tar.gz" \
    && cd "dar-${DAR_VERSION}" \
    && PKG_CONFIG_PATH=/usr/local/lib/pkgconfig \
       LDFLAGS='-Wl,-rpath,/usr/local/lib' \
       ./configure \
         --prefix=/usr/local \
         --disable-static \
         --enable-shared \
         --disable-dependency-tracking \
         --disable-nls \
         --enable-libz-linking \
         --disable-libbz2-linking \
         --disable-liblzo2-linking \
         --disable-libxz-linking \
         --enable-libzstd-linking \
         --enable-liblz4-linking \
         --disable-libgcrypt-linking \
         --disable-librhash-linking \
         --disable-nodump-flag \
         --disable-librsync-linking \
         --disable-libcurl-linking \
         --disable-libssh-linking \
         --disable-build-html \
         --disable-upx \
         --disable-gpgme-linking \
         --disable-libargon2-linking \
         --disable-python-binding \
         --enable-threadar \
    && make -j2 \
    && make install-strip \
    && LD_LIBRARY_PATH=/usr/local/lib /usr/local/bin/dar -V > /tmp/dar-version.txt 2>&1 \
    && cat /tmp/dar-version.txt \
    && grep -E 'gzip compression \(libz\) +: YES$' /tmp/dar-version.txt \
    && grep -E 'zstd compression \(libzstd\) +: YES$' /tmp/dar-version.txt \
    && grep -E 'lz4 compression \(liblz4\) +: YES$' /tmp/dar-version.txt \
    && grep -E 'xz compression \(liblzma\) +: NO$' /tmp/dar-version.txt \
    && grep -F 'Multiple threads (libthreads): YES (1.6.1' /tmp/dar-version.txt

# Ship the exact corresponding source and license texts with the binaries.
RUN install -D -m 0644 \
      "/build/dar-${DAR_VERSION}.tar.gz" \
      "/opt/runtime-docs/tapescale/sources/dar-${DAR_VERSION}.tar.gz" \
    && install -D -m 0644 \
      "/build/libthreadar-${LIBTHREADAR_VERSION}.tar.gz" \
      "/opt/runtime-docs/tapescale/sources/libthreadar-${LIBTHREADAR_VERSION}.tar.gz" \
    && install -D -m 0644 \
      "/build/dar-${DAR_VERSION}/COPYING" \
      /opt/runtime-docs/dar/COPYING \
    && install -D -m 0644 \
      "/build/libthreadar-${LIBTHREADAR_VERSION}/COPYING" \
      /opt/runtime-docs/libthreadar/COPYING \
    && install -D -m 0644 \
      "/build/libthreadar-${LIBTHREADAR_VERSION}/COPYING.LESSER" \
      /opt/runtime-docs/libthreadar/COPYING.LESSER

FROM debian:bookworm-20250721-slim@sha256:2424c1850714a4d94666ec928e24d86de958646737b1d113f5b2207be44d37d8 AS runtime
ARG TAPESCALE_VERSION=0.2.0-alpha.3
ARG TAPESCALE_REVISION=unknown
ARG DEBIAN_SNAPSHOT=20250730T000000Z
LABEL org.opencontainers.image.title="TapeScale" \
      org.opencontainers.image.description="Safety-first LTO backup control plane for TrueNAS" \
      org.opencontainers.image.version="${TAPESCALE_VERSION}" \
      org.opencontainers.image.revision="${TAPESCALE_REVISION}" \
      org.opencontainers.image.source="https://github.com/lilrags16/TapeScale" \
      org.opencontainers.image.licenses="Apache-2.0 AND GPL-2.0-or-later AND LGPL-3.0-or-later"

RUN printf '%s\n' \
      "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT}/ bookworm main" \
      "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT}/ bookworm-security main" \
      > /etc/apt/sources.list.d/tapescale-debian-snapshot.list \
    && printf '%s\n' \
      'Dir::Etc::sourcelist "/etc/apt/sources.list.d/tapescale-debian-snapshot.list";' \
      'Dir::Etc::sourceparts "-";' \
      > /etc/apt/apt.conf.d/99tapescale-snapshot \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
         ca-certificates \
         curl \
         libgcc-s1=12.2.0-14+deb12u1 \
         liblz4-1=1.9.4-1 \
         libstdc++6=12.2.0-14+deb12u1 \
         libzstd1=1.5.4+dfsg2-5 \
         mt-st \
         tzdata \
         zlib1g=1:1.2.13.dfsg-1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 tapescale \
    && useradd --system --uid 10001 --gid tapescale --home-dir /var/lib/tapescale tapescale \
    && install -d -m 0770 -o tapescale -g root \
         /var/lib/tapescale \
         /var/lib/tapescale/recovery \
         /var/lib/tapescale/spool

COPY --from=dar-builder /usr/local/bin/ /usr/local/bin/
COPY --from=dar-builder /usr/local/etc/darrc /usr/local/etc/darrc
COPY --from=dar-builder /usr/local/lib/ /usr/local/lib/
COPY --from=dar-builder /opt/runtime-docs/ /usr/share/doc/
COPY --from=builder /build/target/release/tapescale-api /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-scheduler /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-worker /usr/local/bin/
COPY --from=builder /build/target/release/tapescale-recovery /usr/local/bin/
COPY web /app/web
COPY docs /app/docs
COPY LICENSE THIRD_PARTY_NOTICES.md /app/

RUN ldconfig \
    && dar -V

USER 10001:10001
WORKDIR /app
ENV TAPESCALE_BIND=0.0.0.0:8080 \
    TAPESCALE_WEB_DIR=/app/web \
    RUST_LOG=tapescale=info,tower_http=info
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "http://127.0.0.1:8080/health/ready"]
ENTRYPOINT ["/usr/local/bin/tapescale-api"]

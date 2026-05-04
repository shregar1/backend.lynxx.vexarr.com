# syntax=docker/dockerfile:1.6
#
# Multi-stage build for the Lynxx gateway. The same image graph produces
# two named final stages — `gateway` and `ingress` — each holding only the
# binary it needs.
#
#   docker build --target gateway -t lynxx-gateway .
#   docker build --target ingress -t lynxx-ingress .

FROM rust:1.83-slim-bookworm AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# ---- Cargo dependency cache layer -------------------------------------------
# Copy only the manifests first and build a stub binary so the dependency
# graph compiles into a layer that survives source edits.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo "fn main(){}" > src/main.rs \
    && cargo build --release --bin lynx-gateway || true

# ---- Real build -------------------------------------------------------------
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release --bin lynx-gateway --bin lynxx-ingress

# ---- Shared runtime base ----------------------------------------------------
FROM debian:bookworm-slim AS runtime-base
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -r -u 10001 -m lynxx

# ---- Gateway image ----------------------------------------------------------
FROM runtime-base AS gateway
COPY --from=builder /build/target/release/lynx-gateway /usr/local/bin/
COPY --from=builder /build/migrations /etc/lynxx/migrations
COPY config.toml /etc/lynxx/config.toml
USER lynxx
WORKDIR /etc/lynxx
EXPOSE 9000 9001
ENTRYPOINT ["/usr/local/bin/lynx-gateway"]

# ---- Ingress controller image ----------------------------------------------
FROM runtime-base AS ingress
COPY --from=builder /build/target/release/lynxx-ingress /usr/local/bin/
USER lynxx
ENTRYPOINT ["/usr/local/bin/lynxx-ingress"]

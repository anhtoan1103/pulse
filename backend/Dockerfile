# Builds both Pulse backend binaries (api, worker) from one codebase, per
# docs/pulse-architecture.md #5 — same image, different `command:` in
# docker-compose.yml selects which entrypoint runs in each container.

FROM rust:1-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies separately from source so `cargo build` doesn't
# re-fetch/re-build the whole dependency tree on every source change.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src/bin \
    && echo "fn main() {}" > src/bin/api.rs \
    && echo "fn main() {}" > src/bin/worker.rs \
    && echo "" > src/lib.rs \
    && cargo build --release \
    && rm -rf src

COPY . .
# Touch sources so cargo doesn't reuse the dummy-main objects from the cache step above.
RUN touch src/lib.rs src/bin/api.rs src/bin/worker.rs && cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/false pulse
WORKDIR /app
COPY --from=builder /app/target/release/api ./api
COPY --from=builder /app/target/release/worker ./worker
COPY migrations ./migrations
USER pulse

# docker-compose overrides `command:` per service (api vs worker).
CMD ["./api"]

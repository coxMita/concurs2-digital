# syntax=docker/dockerfile:1

# ---- Build -------------------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src
# Use the committed .sqlx/ query data: no database needed at build time.
ENV SQLX_OFFLINE=true CARGO_TERM_COLOR=never

# 1) Build dependencies only (layer cached until Cargo.toml/Cargo.lock change).
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

# 2) Build the application.
COPY sqlx.toml ./
COPY .sqlx ./.sqlx
COPY migrations ./migrations
COPY templates ./templates
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# ---- Runtime -----------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /app app \
    && mkdir -p /data/uploads \
    && chown -R app:app /data
WORKDIR /app
COPY --from=build /src/target/release/concurs /app/concurs
COPY static /app/static
# Argon2 allocates ~19 MB per hash. Pin glibc's mmap threshold so these buffers are
# always mmapped and returned to the OS on free (otherwise a burst of logins grows
# per-thread arenas), and cap the number of arenas.
ENV BIND_ADDR=0.0.0.0:8080 \
    STATIC_DIR=/app/static \
    UPLOAD_DIR=/data/uploads \
    RUST_LOG=info,sqlx=warn,tower_http=warn \
    MALLOC_MMAP_THRESHOLD_=131072 \
    MALLOC_ARENA_MAX=2
USER app
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=5 CMD ["/app/concurs", "healthcheck"]
CMD ["/app/concurs"]

# syntax=docker/dockerfile:1

# ---- builder ----
FROM rust:1-bookworm AS builder
WORKDIR /app

# Copy the full source. Templates (Askama) and static assets are pulled in at
# compile time via macros / include_bytes!, so they must be present to build.
COPY . .

# BuildKit cache mounts keep the cargo registry and target dir warm across
# builds. The binary is copied out of the cache mount in the same RUN so it
# survives into the next stage.
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release && \
    cp target/release/ks-notes /usr/local/bin/ks-notes

# ---- runtime ----
FROM debian:bookworm-slim AS runtime

# Non-root user; data lives in a volume it owns.
RUN useradd --system --create-home --uid 10001 ksnotes && \
    mkdir -p /data && chown ksnotes:ksnotes /data

COPY --from=builder /usr/local/bin/ks-notes /usr/local/bin/ks-notes

USER ksnotes
WORKDIR /data
VOLUME ["/data"]

# Listen on all interfaces inside the container; the DB lives on the volume.
ENV KSNOTES_BIND=0.0.0.0 \
    KSNOTES_PORT=5230 \
    KSNOTES_DB_PATH=/data/ks-notes.db \
    RUST_LOG=info

EXPOSE 5230

ENTRYPOINT ["ks-notes"]

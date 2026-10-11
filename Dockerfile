# Dashboard image: the read-only `web` bin serving the built SPA, plus `backfill` and
# `morning` so the container can refresh the morning report itself (scripts/serve.sh).

FROM node:22-bookworm-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
RUN cargo build --release --bin web --bin backfill --bin morning

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /app/target/release/web /app/target/release/backfill /app/target/release/morning /usr/local/bin/
COPY --from=frontend /app/frontend/out ./frontend/out
COPY config ./config
COPY scripts/serve.sh ./scripts/serve.sh
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
CMD ["./scripts/serve.sh"]

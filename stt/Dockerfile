# --- Build stage ---
FROM rust:latest AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml ./
COPY src/ ./src/
RUN cargo build --release

# --- Runtime stage ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/varg-media /usr/local/bin/varg-media
EXPOSE 5000
CMD ["varg-media"]

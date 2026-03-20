# Multi-stage Dockerfile for the Varg Matrix Agent
#
# Stage 1: Compile Varg source to Rust to native binary
# Stage 2: Minimal runtime image with just the binary

# --- Stage 1: Build ---
FROM rust:1.82-bookworm AS builder

# Install vargc compiler
# Option A: Download pre-built binary (Windows only currently)
# Option B: Build from source
# For now, we clone and build the compiler, then use it to compile the agent.

WORKDIR /build

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Clone and build the Varg compiler
RUN git clone https://github.com/LupusMalusDeviant/VARG.git /build/varg-compiler
WORKDIR /build/varg-compiler/varg-compiler
RUN cargo build --release -p vargc
RUN cp target/release/vargc /usr/local/bin/vargc

# Copy agent source code
WORKDIR /build/agent
COPY src/ ./src/
COPY config/ ./config/

# Compile the Varg agent to a native binary
# vargc build creates a .vargc_cache/ with a Cargo project, then builds it
RUN vargc build src/main.varg || true

# If vargc build produces the binary, copy it
# Fallback: use the generated Rust source and compile directly
RUN if [ -f main ]; then \
        cp main /build/varg_agent; \
    elif [ -d .vargc_cache ]; then \
        cd .vargc_cache && cargo build --release && \
        cp target/release/* /build/varg_agent 2>/dev/null || true; \
    fi

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    wget \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /build/varg_agent /app/varg_agent
# Also copy vargc for self-extending capabilities (write_tool, compile_varg)
COPY --from=builder /usr/local/bin/vargc /usr/local/bin/vargc

# Copy source and config for tool management
COPY src/ /app/src/
COPY config/ /app/config/

# Create data directory for persistence
RUN mkdir -p /app/data /app/src/tools

# Non-root user
RUN useradd -m -r varg && chown -R varg:varg /app
USER varg

ENTRYPOINT ["/app/varg_agent"]

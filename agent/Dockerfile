# =============================================================================
# Multi-stage build: Compile vargc from source, then compile agent.varg
# Stage 3 now includes vargc + Rust toolchain for runtime tool compilation
# =============================================================================

# --- Stage 1: Build the Varg compiler from source ---
FROM rust:latest AS vargc-builder

WORKDIR /build

# Cache-bust: bump to force fresh clone (v0.9.0+fix4 = self-bug fix)
ARG VARG_VERSION=v0.9.0-fix4
RUN apt-get update && apt-get install -y git pkg-config libssl-dev && \
    git clone --depth 1 https://github.com/LupusMalusDeviant/VARG.git

WORKDIR /build/VARG/varg-compiler
RUN cargo build --release -p vargc && \
    cp target/release/vargc /usr/local/bin/vargc

# --- Stage 2: Compile the Varg agent ---
FROM rust:latest AS agent-builder

RUN rustup component add rustfmt

COPY --from=vargc-builder /usr/local/bin/vargc /usr/local/bin/vargc
COPY --from=vargc-builder /build/VARG/varg-compiler/crates /app/crates

WORKDIR /app

COPY agent.varg .

# Build the agent - vargc looks for crates at /app/crates/
RUN vargc build agent.varg

# --- Stage 3: Runtime with vargc for self-extending tools ---
FROM rust:latest AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 \
    && rustup component add rustfmt \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy compiled agent binary + source (for self-knowledge graph)
COPY --from=agent-builder /app/agent /app/agent
COPY --from=agent-builder /app/agent.varg /app/agent.varg

# Copy vargc compiler + crates for runtime tool compilation
COPY --from=vargc-builder /usr/local/bin/vargc /usr/local/bin/vargc
COPY --from=vargc-builder /build/VARG/varg-compiler/crates /app/crates

RUN mkdir -p /app/data /app/tools

# Run from /app/data so graph_open() creates .graph.db files in the persistent volume
CMD ["sh", "-c", "cd /app/data && /app/agent"]

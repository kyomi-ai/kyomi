# Dockerfile for ChartML Chart Renderer Service
# Node.js service that renders ChartML charts to PNG images using JSDOM + D3 + sharp
#
# Uses node:20-slim (Debian) instead of Alpine because @duckdb/node-api
# requires glibc (not available on musl/Alpine).
#
# Multi-stage build: build tools stay in builder, only runtime libs in final image.

# --- Builder stage: compile native modules ---
FROM node:20-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 \
    make \
    g++ \
    libcairo2-dev \
    libjpeg-dev \
    libpango1.0-dev \
    libgif-dev \
    libpixman-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app/apps/chart-renderer

# Copy shared package (matching monorepo relative path for file: reference)
COPY packages/chartml-transform/ /app/packages/chartml-transform/

COPY apps/chart-renderer/package.json ./

# Install dependencies (native modules compiled here with build tools)
RUN npm install --omit=dev

# --- Runtime stage: minimal image ---
FROM node:20-slim

# Install only runtime libraries (no compilers/headers) + fonts + weasyprint
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 \
    python3-pip \
    libcairo2 \
    libjpeg62-turbo \
    libpango-1.0-0 \
    libpangocairo-1.0-0 \
    libgif7 \
    libpixman-1-0 \
    fontconfig \
    fonts-dejavu-core \
    fonts-liberation \
    fonts-freefont-ttf \
    && fc-cache -f \
    && rm -rf /var/lib/apt/lists/* \
    && pip3 install --break-system-packages weasyprint

WORKDIR /app/apps/chart-renderer

# Copy pre-built node_modules from builder
COPY --from=builder /app /app

# Copy application source
COPY apps/chart-renderer/src/ ./src/

# Copy quackstats native extension (built for linux_amd64)
COPY apps/chart-renderer/extensions/ ./extensions/
ENV QUACKSTATS_EXTENSION_PATH=/app/apps/chart-renderer/extensions/quackstats.duckdb_extension

# Run as non-root user
USER node

EXPOSE 3030

HEALTHCHECK --interval=30s --timeout=10s --retries=3 \
    CMD node -e "require('http').get('http://localhost:3030/health', (r) => process.exit(r.statusCode === 200 ? 0 : 1))"

CMD ["node", "src/server.js"]

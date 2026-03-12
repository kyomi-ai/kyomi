#######################################
# Build stage
#######################################
FROM node:20-alpine as builder

WORKDIR /app

# IMPORTANT: Copy ChartML from local repo FIRST (required for npm ci file: dependencies)
# Build context is parent of kyomi, so chartml is at ./chartml
COPY chartml /chartml

# Copy package files ONLY first (for better layer caching)
# Changes to source code won't invalidate the npm install layer
COPY kyomi/apps/frontend/package.json ./

# Install dependencies (now chartml exists for file: dependencies)
# NOTE: Cannot use package-lock.json with file: dependencies in Docker
# The lockfile contains host-specific paths that don't work in the container
# Must install all deps (including dev) for build to work
RUN npm install

# Copy config files needed for build
COPY kyomi/apps/frontend/vite.config.js ./
COPY kyomi/apps/frontend/tailwind.config.js ./
COPY kyomi/apps/frontend/postcss.config.js ./
COPY kyomi/apps/frontend/index.html ./

# Copy source code LAST (most frequently changed)
COPY kyomi/apps/frontend/src ./src
COPY kyomi/apps/frontend/public ./public

# Build for production
RUN npm run build

#######################################
# Production image - simple static file server
#######################################
FROM node:20-alpine

# Install serve (simple static file server)
RUN npm install -g serve

WORKDIR /app

# Copy built files from builder
COPY --from=builder /app/dist ./dist

# Run as non-root user
USER node

# Expose port (internal to Docker network)
EXPOSE 3000

# Serve static files
CMD ["serve", "-s", "dist", "-l", "3000"]

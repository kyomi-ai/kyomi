# Build stage — compile static musl binary
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY apps/analytics-collector/ .
RUN cargo build --release --target x86_64-unknown-linux-musl

# Runtime stage — scratch container with just the binary + transforms
FROM scratch
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/kyomi-collector /collector
COPY --from=builder /app/transforms/ /transforms/
USER 1000
EXPOSE 8003
ENV TRANSFORMS_DIR=/transforms
ENTRYPOINT ["/collector"]

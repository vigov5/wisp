# Stage 1: Build
FROM rust:1.94-slim AS builder

WORKDIR /usr/src/wisp
COPY . .

# Build the server package
RUN cargo build --release -p wisp-server

# Stage 2: Runtime
FROM debian:bookworm-slim

WORKDIR /app

# Install necessary libraries (like OpenSSL if needed)
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy the binary from the builder stage
COPY --from=builder /usr/src/wisp/target/release/wisp-server /app/wisp-server

EXPOSE 8787

# Run the server
CMD ["/app/wisp-server", "serve", "--listen", "0.0.0.0:8787"]

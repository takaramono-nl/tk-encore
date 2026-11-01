# syntax=docker/dockerfile:1

FROM rust:1.94-alpine AS build
RUN apk add --no-cache build-base cmake perl
WORKDIR /src

# Compile dependencies once; this layer is reused until Cargo.toml/Cargo.lock
# change. The stub sources keep `cargo build` from needing the real crate.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
 && echo 'pub fn _stub() {}' > src/lib.rs \
 && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src

COPY migrations ./migrations
COPY src ./src
# The stub build above leaves newer artifacts than the copied sources, so
# Cargo would consider the real crate fresh. Bump the mtimes to force a
# rebuild of the crate while keeping the compiled dependencies cached.
RUN find src -type f -exec touch {} + && cargo build --release --locked

FROM alpine:3.22
RUN apk add --no-cache ca-certificates tzdata
COPY --from=build /src/target/release/encore /usr/local/bin/encore
RUN mkdir -p /data
ENV ENCORE_DB=/data/encore.db
WORKDIR /data

# Encore is a batch CLI, not a server. Stay alive so an external scheduler can
# exec tasks into the container; the ledger lives on /data.
CMD ["sleep", "infinity"]

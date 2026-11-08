# Образ с компилятором Orbitron и обоими бэкендами (clang + JDK).
#
#   docker build -t orbitron .
#   docker run --rm -v "$PWD":/work -w /work orbitron run examples/01_hello.ot

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libc6-dev openjdk-17-jdk-headless \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/orbitron /usr/local/bin/orbitron
COPY stdlib /usr/local/share/orbitron/stdlib
ENTRYPOINT ["orbitron"]

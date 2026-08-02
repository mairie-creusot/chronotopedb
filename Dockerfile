# chronotope-server — binaire d'integration HTTP/WebSocket (voir
# crates/chronotope-server). Meme recette que Dockerfile.pawchat du fork
# SpacetimeDB de PawChat (mairie-creusot/SpacetimeDB) : cross-compile musl
# statique + strip + LTO fat pour une image finale minimale, cargo-chef pour
# ne recompiler les dependances que si Cargo.lock change.

ARG CARGO_STRIP=symbols
ARG CARGO_LTO=fat
ARG CARGO_CODEGEN_UNITS=1

FROM rust:1.93.0 AS chef
RUN rust_target=$(rustc -vV | awk '/^host:/{ print $2 }') && \
  curl https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-$rust_target.tgz -fL | tar xz -C $CARGO_HOME/bin
RUN cargo binstall -y cargo-chef@0.1.70
RUN rustup target add x86_64-unknown-linux-musl
RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
WORKDIR /usr/src/app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /usr/src/app/recipe.json .
ENV CARGO_INCREMENTAL=0
ARG CARGO_STRIP
ARG CARGO_LTO
ARG CARGO_CODEGEN_UNITS
ENV CARGO_PROFILE_RELEASE_STRIP=${CARGO_STRIP} \
    CARGO_PROFILE_RELEASE_LTO=${CARGO_LTO} \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=${CARGO_CODEGEN_UNITS}
RUN cargo chef cook --release -p chronotope-server --recipe-path recipe.json --target x86_64-unknown-linux-musl
COPY . .
RUN cargo build --release -p chronotope-server --locked --target x86_64-unknown-linux-musl

# Pas de ca-certificates ni libgcc : le binaire est static-pie musl et ne
# fait aucun appel HTTPS sortant (serveur HTTP entrant uniquement) — meme
# raisonnement que le Dockerfile.pawchat de SpacetimeDB, verifie ici plutot
# que suppose (aucune dependance TLS dans Cargo.toml de chronotope-server).
FROM alpine:3.20 AS runtime
RUN addgroup -S chronotope && adduser -S -G chronotope chronotope
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/chronotope-server /usr/local/bin/

EXPOSE 3200
ENV RUST_LOG=info
USER chronotope
# wget vient de busybox, deja present dans l'image de base alpine:3.20 —
# aucun paquet supplementaire, donc aucun cout sur le budget de 80 Mo.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:3200/health || exit 1
ENTRYPOINT ["chronotope-server"]

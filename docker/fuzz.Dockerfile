# Toolchain image for the fuzz targets in src-tauri/fuzz: cargo-fuzz on a
# nightly Rust, which its coverage instrumentation needs, for fuzzing only. The
# crate itself is still built, linted and tested on the stable toolchain alone
# (src-tauri/rust-toolchain.toml, linux-check.Dockerfile).
# Build:  docker build -t luna-fuzz -f docker/fuzz.Dockerfile docker
# Usage:  see scripts/fuzz.ps1
# The base is linux-check's, by the same tag and digest (the gate checks the
# Rust tag against rust-toolchain.toml); the nightly is pinned by date, here and
# in src-tauri/fuzz/rust-toolchain.toml, and scripts/fuzz.ps1 fails when the two
# disagree.
FROM rust:1.99.1-slim-trixie@sha256:7eab7ce19273ce785e0ca7578bdf7bc1fcbae621775a8197379a04c9f6e03cd5

# What the crate links on Linux, as in linux-check.Dockerfile; g++ builds the
# libFuzzer runtime libfuzzer-sys compiles from source.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libwebkit2gtk-4.1-dev libdbus-1-dev g++ \
    && rm -rf /var/lib/apt/lists/*

# rustc 1.101.0-nightly (12433c307 2026-10-24); nightly-2026-11-01 fails to
# build zvariant_utils 4.0.0 (E0391, a cycle computing "needs async drop").
# cargo-fuzz is built by the stable toolchain from its own lockfile.
RUN rustup toolchain install nightly-2026-10-25 --profile minimal \
    && cargo install cargo-fuzz --version 0.13.2 --locked \
    && rm -rf /usr/local/cargo/registry

# The targets need no frontend; the build only wants a folder to embed. See
# linux-check.Dockerfile for why it is set here.
RUN mkdir /no-frontend
ENV TAURI_CONFIG='{"build":{"frontendDist":"/no-frontend"}}'

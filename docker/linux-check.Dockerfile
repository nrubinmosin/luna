# Toolchain image for building the crate for Linux and running its unit tests:
# the Unix code (keeper/, procs/unix.rs and parts of pty/) that the Windows test
# run never compiles.
# Build:  docker build -t luna-linuxcheck -f docker/linux-check.Dockerfile docker
# Usage:  see scripts/linux-check.ps1
# Rust is pinned here and in src-tauri/rust-toolchain.toml; the gate checks that
# they agree (scripts/gate.ps1). The digest holds the tag to one build of it.
FROM rust:1.99.1-slim-trixie@sha256:7eab7ce19273ce785e0ca7578bdf7bc1fcbae621775a8197379a04c9f6e03cd5

# What the crate links on Linux: webkit2gtk brings GTK and libsoup along, tao
# needs libdbus. git, sh, sleep and kill are what the tests themselves run.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libwebkit2gtk-4.1-dev libdbus-1-dev \
    git procps \
    && rm -rf /var/lib/apt/lists/*

# The formatter and lint gates (cargo fmt --check, cargo clippy) run here too,
# and the image ships neither. The Windows target is never built here, but
# src-tauri/rust-toolchain.toml lists it, and rustup would otherwise fetch it
# into every throwaway container that runs cargo there.
RUN rustup component add rustfmt clippy     && rustup target add x86_64-pc-windows-msvc

# The tests need no frontend, and the build only wants a folder to embed. Here
# and not in the script's command: Windows PowerShell breaks up a native argument
# with a double quote in it, and JSON has no way around them.
RUN mkdir /no-frontend
ENV TAURI_CONFIG='{"build":{"frontendDist":"/no-frontend"}}'

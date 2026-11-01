# Toolchain image for building the crate for Linux and running its unit tests:
# the Unix code (keeper.rs, parts of procs.rs and pty.rs) that the Windows test
# run never compiles.
# Build:  docker build -t luna-linuxcheck -f docker/linux-check.Dockerfile docker
# Usage:  see linux-check.ps1
FROM rust:1-slim-trixie

# What the crate links on Linux: webkit2gtk brings GTK and libsoup along, tao
# needs libdbus. git, sh, sleep and kill are what the tests themselves run.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libwebkit2gtk-4.1-dev libdbus-1-dev \
    git procps \
    && rm -rf /var/lib/apt/lists/*

# Toolchain image for cross-compiling the Windows build from Linux.
# Build:  scripts/lib.ps1 Build-Image, or docker build -t luna-winbuild
#           -f docker/windows-build.Dockerfile docker
# Usage:  see scripts/build-windows.ps1
#
# Every version is pinned: Rust here and in src-tauri/rust-toolchain.toml, Node
# here and in the repo's .node-version, cargo-xwin below. The gate checks that
# the pins agree (scripts/gate.ps1), and the digests hold each tag to one build.

# Node comes from the official image of the exact version, not from a distro or
# NodeSource channel that moves under the same name. The gate's frontend steps
# run in this same image (scripts/lib.ps1 Get-NodeImage).
FROM node:24.22.2-trixie-slim@sha256:0820eaa85e07d0490446dd01d14707e17e6108a36988cf374899cd057eb3812d AS node

# Trixie for NSIS 3.11: the tauri 2.12 installer script includes
# Win\RestartManager.nsh, which bookworm's NSIS 3.08 does not ship.
FROM rust:1.99.1-trixie@sha256:8a4e90c7946b74626488b1446f712c0fe0ed1a596cbd3d871684bc9c5433ae07

RUN apt-get update && apt-get install -y --no-install-recommends \
    nsis lld llvm clang curl ca-certificates \
    libayatana-appindicator3-dev \
    && rm -rf /var/lib/apt/lists/*

COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
    && ln -s ../lib/node_modules/corepack/dist/corepack.js /usr/local/bin/corepack \
    && corepack enable

# rustfmt and clippy for the lint gate over the Windows-only code, which the
# Linux image never compiles; what src-tauri/rust-toolchain.toml lists, so
# rustup has nothing to fetch at build time.
RUN rustup component add rustfmt clippy \
    && rustup target add x86_64-pc-windows-msvc \
    && cargo install --locked cargo-xwin --version 0.23.1

# cargo-xwin downloads the Windows SDK/CRT on first use into this dir;
# mount a named volume over it to cache between runs.
ENV XWIN_CACHE_DIR=/xwin-cache

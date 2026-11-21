# Toolchain image for cross-compiling the Windows build from Linux.
# Build:  scripts/lib.ps1 Build-Image, or docker build -t luna-winbuild
#           -f docker/windows-build.Dockerfile docker
# Usage:  see scripts/build-windows.ps1
#
# The gate checks the Rust pin against rust-toolchain.toml. Debian trixie
# provides the NSIS 3.11 headers used by the installer.
FROM rust:1.100.0-trixie@sha256:e19840879045c951c905d0eaa9887f3439c62000e429963371b376bac8d38c31

RUN apt-get update && apt-get install -y --no-install-recommends \
    nsis lld llvm clang curl ca-certificates \
    libayatana-appindicator3-dev \
    && rm -rf /var/lib/apt/lists/*

# rustfmt and clippy for the lint gate over the Windows-only code, which the
# Linux image never compiles; what src-tauri/rust-toolchain.toml lists, so
# rustup has nothing to fetch at build time.
RUN rustup component add rustfmt clippy \
    && rustup target add x86_64-pc-windows-msvc \
    && cargo install --locked cargo-xwin --version 0.23.1

# cargo-xwin downloads the Windows SDK/CRT on first use into this dir;
# mount a named volume over it to cache between runs.
ENV XWIN_CACHE_DIR=/xwin-cache

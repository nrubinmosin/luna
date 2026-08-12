# Toolchain image for cross-compiling the Windows build from Linux.
# Build:  docker build -t llm-desktop-winbuild -f docker/windows-build.Dockerfile docker
# Usage:  see build-windows.ps1
FROM rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    nsis lld llvm clang curl ca-certificates \
    libayatana-appindicator3-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs \
    && corepack enable \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-pc-windows-msvc \
    && cargo install --locked cargo-xwin

# cargo-xwin downloads the Windows SDK/CRT on first use into this dir;
# mount a named volume over it to cache between runs.
ENV XWIN_CACHE_DIR=/xwin-cache

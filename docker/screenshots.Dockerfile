# Toolchain image for the README screenshots: node to serve the frontend and a
# headless Chromium to shoot it. No Rust and no Tauri — the shots are of the UI
# alone, standing on the ?demo fixture.
# Build:  docker build -t luna-shots -f docker/screenshots.Dockerfile docker
# Usage:  see screenshots.ps1
FROM node:22-bookworm-slim

# fonts-jetbrains-mono so the terminals read right even if Google Fonts is
# unreachable; the UI's own Tahoma is mounted in from the host at run time.
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium fontconfig fonts-jetbrains-mono fonts-dejavu-core curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN corepack enable

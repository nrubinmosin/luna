# Toolchain image for the README screenshots: node to serve the frontend and a
# headless Chromium to shoot it. No Rust and no Tauri — the shots are of the UI
# alone, standing on the ?demo fixture.
# Build:  docker build -t luna-shots -f docker/screenshots.Dockerfile docker
# Usage:  see scripts/screenshots.ps1
# Node at the exact version of the repo's .node-version, like the Windows image;
# the gate checks that they agree (scripts/gate.ps1). The digest holds the tag to
# one build of it.
FROM node:26.13.1-bookworm-slim@sha256:5df79989ed685e09f0a98019db4cebfb22fb9df1a53c0161664a011f7ff6b0f1

# The terminals' JetBrains Mono ships with the app (public/fonts); the UI's
# own Windows faces are mounted in from the host at run time.
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium fontconfig fonts-jetbrains-mono fonts-dejavu-core curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# corepack, which Node no longer ships, at the version
# docker/windows-build.Dockerfile installs; the gate checks that they agree.
RUN npm install --global --no-fund --no-audit --no-update-notifier corepack@0.36.0 \
    && corepack enable

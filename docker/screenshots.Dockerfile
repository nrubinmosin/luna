# Toolchain image for the README screenshots: node to serve the frontend and a
# headless Chromium to shoot it. No Rust and no Tauri — the shots are of the UI
# alone, standing on the ?demo fixture.
# Build:  docker build -t luna-shots -f docker/screenshots.Dockerfile docker
# Usage:  see scripts/screenshots.ps1
# Node at the exact version of the repo's .node-version, like the Windows image;
# the gate checks that they agree (scripts/gate.ps1). The digest holds the tag to
# one build of it.
FROM node:26.12.1-bookworm-slim@sha256:88e480776c6149ff1d098cd304f44bf18057fb5d284a3329b78162d29821e28c

# The terminals' JetBrains Mono ships with the app (public/fonts); the UI's
# own Windows faces are mounted in from the host at run time.
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium fontconfig fonts-jetbrains-mono fonts-dejavu-core curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN corepack enable

# check=skip=InvalidDefaultArgInFrom
# Toolchain image for the README screenshots: node to serve the frontend and a
# headless Chromium to shoot it. No Rust and no Tauri — the shots are of the UI
# alone, standing on the ?demo fixture.
# Build:  docker build -t luna-shots --build-arg NODE_VERSION=<.node-version>
#           -f docker/screenshots.Dockerfile docker
# Usage:  see scripts/screenshots.ps1 (scripts/lib.ps1 Build-Image passes the arg)
# Node at the exact version of the repo's .node-version, like the Windows image.
# No default, on purpose (hence the check= line at the top): a build without
# the argument fails rather than picking some other Node.
ARG NODE_VERSION
FROM node:${NODE_VERSION}-bookworm-slim

# The terminals' JetBrains Mono ships with the app (public/fonts); the UI's
# own Windows faces are mounted in from the host at run time.
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium fontconfig fonts-jetbrains-mono fonts-dejavu-core curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN corepack enable

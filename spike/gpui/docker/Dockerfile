# The spike's toolchain: Luna's own Windows cross-build image
# (docker/windows-build.Dockerfile in the repo: Rust, clang, lld, llvm,
# cargo-xwin, all pinned there), passed in as BASE by build.ps1. GPUI needs
# nothing more: the patched gpui-pre-windows compiles its HLSL at runtime
# (README.md), so no fxc.exe; gpui-pre's manifest resource goes through llvm-rc.
ARG BASE
FROM ${BASE}
ENV XWIN_CACHE_DIR=/xwin-cache

# The image every release artifact is built in, and the suite run in
# (`docs/design/roadmap-P29-releases.md`, "The build image"). Built and run by
# `scripts/release.py`, never by hand: it names the context this file expects
# and tags the image by its inputs.
#
# Built on the architecture it runs on: an x86-64 host's image cross-builds the
# arm64 target, and an arm64 host's runs that target's archived suite, so
# the cross toolchain below is installed on x86-64 alone.
#
# **The digest below is the one pin of what a release links.** It names the
# base image, its glibc — the floor every release links against — and, through
# the snapshot it was built from, every Debian package installed here, the C
# compilers among them (`.claude/skills/upgrade-deps/SKILL.md`). Rust is
# `rust-toolchain.toml`'s; the suite's tools are `mise.toml`'s, as pinned there.
FROM debian:trixie@sha256:913f6706df59a68922d1dd08f78c2476560a8d367897200a6005b00e5f67c2d5

# **apt reads the snapshot the image was built from**, which the image records
# beside each live URI as a comment, so a package installed here is the one
# the digest's day had rather than whatever the archive serves today. A base
# image recording no snapshot fails the build instead of reading the live
# archive. The snapshot's `Release` files have expired by design, hence
# `Check-Valid-Until=false`.
RUN set -eu; \
    f=/etc/apt/sources.list.d/debian.sources; \
    sed -i 's|^# \(http://snapshot\.debian\.org/archive/[a-z-]*/[0-9]*T[0-9]*Z\)$|URIs: \1|; /^URIs: http:\/\/deb\.debian\.org/d' "$f"; \
    test "$(grep -c '^URIs: http://snapshot\.debian\.org/' "$f")" -eq 2; \
    ! grep -q '^URIs: http://deb\.debian\.org' "$f"; \
    echo 'Acquire::Check-Valid-Until "false";' > /etc/apt/apt.conf.d/99snapshot

# Debian's gcc for the C dependencies. `rustup`, `curl` and `ca-certificates`
# fetch the pinned Rust and `mise`; `binutils`' `readelf` reads a binary's
# glibc needs; `xz-utils`, `util-linux` (`unshare`), `procps` (`kill`), `git`
# and `python3` are what the suite runs beside the binaries it builds. `make`
# builds jemalloc for the register's `jemalloc` allocator leg
# (`scripts/measure.py`, `ALLOCATOR_LEGS`), which is built here beside the
# shipped binary; no release artifact links it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils ca-certificates curl gcc git libc6-dev make procps python3 \
        rustup util-linux xz-utils \
    && rm -rf /var/lib/apt/lists/*

# The second target's cross toolchain, on the architecture that builds it: an
# arm64 host has the native one and no use for these.
RUN set -eu; \
    if [ "$(dpkg --print-architecture)" = amd64 ]; then \
        apt-get update; \
        apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu libc6-dev-arm64-cross; \
        rm -rf /var/lib/apt/lists/*; \
    fi

# **The Rust `rust-toolchain.toml` pins**, installed by reading that file as a
# checkout's `rustup` does, so the image and the host cannot name two
# compilers. Its homes are read-only to the builder: `scripts/release.py` runs
# as the invoking user with a `CARGO_HOME` of its own.
ENV RUSTUP_HOME=/usr/local/rustup
COPY rust-toolchain.toml /opt/toolchain/rust-toolchain.toml
RUN cd /opt/toolchain && rustc --version && cargo --version \
    && chmod -R a+rX "$RUSTUP_HOME"

# **The tools `mise.toml` pins**, installed by `mise` from that file, so the
# suite and the notices find what a checkout's `mise install` gives it
# (`docs/design/roadmap.md`, "A test may assume the tools `mise` pins").
# `mise` itself is pinned here by version and one checksum per architecture
# (both in the release's `SHASUMS256.txt`), and only installs: each tool is
# linked onto `PATH`, so nothing runs through it.
ARG MISE_VERSION=2026.9.12
ARG MISE_SHA256_X64=e79ae57945034903aee8aa2ea66b4c7ca9cd4f4edd5a8a78a589cbae6d0f428a
ARG MISE_SHA256_ARM64=f344c6961190ed2f68e595ed7cb4f03c36c17812bd608886bec799a3082180ff
ENV MISE_DATA_DIR=/opt/mise MISE_CONFIG_DIR=/opt/mise/config MISE_CACHE_DIR=/opt/mise/cache
COPY mise.toml /opt/toolchain/mise.toml
RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
        amd64) mise_arch=x64; mise_sha="$MISE_SHA256_X64" ;; \
        arm64) mise_arch=arm64; mise_sha="$MISE_SHA256_ARM64" ;; \
        *) echo "no mise build for $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/mise \
        "https://github.com/jdx/mise/releases/download/v${MISE_VERSION}/mise-v${MISE_VERSION}-linux-${mise_arch}"; \
    echo "${mise_sha}  /usr/local/bin/mise" | sha256sum -c -; \
    chmod +x /usr/local/bin/mise; \
    cd /opt/toolchain; \
    mise trust mise.toml; \
    mise install --yes; \
    for tool in cargo-about cargo-nextest cmake uv uvx; do \
        ln -s "$(mise which "$tool")" "/usr/local/bin/$tool"; \
    done; \
    rm -rf "$MISE_CACHE_DIR"; \
    chmod -R a+rX "$MISE_DATA_DIR"

# The arm64 cross build: the linker, and the C compiler, archiver and linker
# every `cc`-driven build script reads by target. On an arm64 host nothing
# builds that target, so these name tools it does not have and nothing reads them.
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar

# The suite's `uv` uses the image's Python, never one it downloads.
ENV UV_PYTHON_DOWNLOADS=never

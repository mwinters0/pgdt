# The container image a release publishes, `ghcr.io/<owner>/<repo>`: one
# manifest per release target, each holding that target's release archive's
# files on a slim Debian (`docs/design/decisions.md`, "D171"). Built by
# `scripts/release_container.py`, never by hand: it stages the context this
# file expects -- one directory per Docker architecture, holding the
# archive's files -- and passes the build arguments below.
#
# **Nothing is compiled or run here**, only copied, so one host builds every
# platform without emulating any. The binary is the archive's byte for byte,
# which the smoke run checks.
#
# **The base is pinned by digest, on the codename `release/Dockerfile`
# builds against**, so the glibc here is at or past the floor the binary
# links (D104); `release_container.py` refuses two codenames. It is bumped
# like that pin (`.claude/skills/upgrade-deps/SKILL.md`).
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2

ARG TARGETARCH
COPY ${TARGETARCH}/pgdt /usr/local/bin/pgdt
COPY ${TARGETARCH}/LICENSE ${TARGETARCH}/THIRD-PARTY-NOTICES ${TARGETARCH}/README.md /usr/share/doc/pgdt/

# A dump is mounted here, and `pgdt` writes its cache beside it. **Root by
# default**: a fixed unprivileged user could not write into a mounted
# directory it does not own, so the manual has the caller pass `--user`.
WORKDIR /data
ENTRYPOINT ["pgdt"]

# What the registry shows and `docker inspect` reads; `source` is also how
# GHCR links the package to its repository.
ARG VERSION
ARG REVISION
ARG SOURCE
LABEL org.opencontainers.image.title="pgdt" \
      org.opencontainers.image.description="Inspect Postgres dumps and query them like parquet" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.source="${SOURCE}" \
      org.opencontainers.image.licenses="Apache-2.0"

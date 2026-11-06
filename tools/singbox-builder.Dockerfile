# The runtime is pure Go; this amd64 image cross-compiles both targets.
# Build from the repository root with --platform=linux/amd64.
FROM --platform=linux/amd64 golang:1.26.8-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git python3 binutils coreutils \
    && rm -rf /var/lib/apt/lists/*

COPY tools/build-singbox.sh /usr/local/bin/build-singbox
RUN chmod 0755 /usr/local/bin/build-singbox
COPY tools/singbox-runtime /usr/local/share/apsp-singbox-runtime
ENV APSP_SINGBOX_PATCHES=/usr/local/share/apsp-singbox-runtime

WORKDIR /build
ENTRYPOINT ["/usr/local/bin/build-singbox"]
CMD ["--help"]

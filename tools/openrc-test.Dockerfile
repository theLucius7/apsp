FROM alpine:3.24.2
RUN apk add --no-cache bash ca-certificates curl iproute2 openrc python3 py3-cryptography minisign util-linux
ENV APSP_OPENRC_SMOKE=1
WORKDIR /src
ENTRYPOINT ["sh", "-c", "python3 tools/test-openrc-smoke.py && exec python3 tools/openrc-smoke.py"]

# syntax=docker/dockerfile:1.7
# clipos worker: job runner (transcode with ffmpeg, killfeed analysis with ONNX Runtime).
#
# Builds the binary from source. deploy.yml builds it on the runner instead (with a warm
# cargo cache) and passes it in as the `bin` stage: --build-context bin=<dir with clipos-worker>

FROM rust:1.98-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p clipos-worker --bin clipos-worker \
    && cp target/release/clipos-worker /usr/local/bin/clipos-worker

FROM scratch AS bin
COPY --from=build /usr/local/bin/clipos-worker /clipos-worker

# Microsoft's official ONNX Runtime build, pinned by checksum; the worker loads it at
# runtime (ORT_DYLIB_PATH) to run the killfeed models.
FROM debian:trixie-slim AS onnxruntime
ARG ORT_VERSION=1.28.2
ADD --checksum=sha256:d7209b8751b27b862b0c76332c2e20e203396edb5dab700ecf4bb485cf147415 \
    https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-x64-${ORT_VERSION}.tgz /tmp/ort.tgz
RUN mkdir -p /opt/onnxruntime \
    && tar -xzf /tmp/ort.tgz -C /opt/onnxruntime --strip-components=2 "onnxruntime-linux-x64-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION}" \
    && ln -s "libonnxruntime.so.${ORT_VERSION}" /opt/onnxruntime/libonnxruntime.so

# ffmpeg is Debian trixie's (7.1), patched through Debian's security updates and not
# pinned, so a rebuild picks the latest. It still carries CVEs Debian has postponed, and
# nothing newer is packaged for trixie or trixie-backports (checked 2026-10-03; forky has
# 9.0). A static build from elsewhere wouldn't be vetted or patched by anyone we rely on.
# The mitigation is in the worker (crates/worker/src/transcode.rs): every ffmpeg and
# ffprobe run on an upload reads only MP4/MOV, MKV/WebM or raw H.264/HEVC, decodes only the
# codecs it lists, opens only the file or its SAS URL, writes no subtitle or data streams
# and stops at the 5-minute cap and a time limit.
FROM debian:trixie-slim
# Upgrade too: the base image lags trixie-security (Trivy, audit.yml).
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home clipos
COPY --from=bin /clipos-worker /usr/local/bin/clipos-worker
COPY --from=onnxruntime /opt/onnxruntime /opt/onnxruntime
ENV BIND_ADDR=0.0.0.0:8081 \
    LOG_FORMAT=json \
    ORT_DYLIB_PATH=/opt/onnxruntime/libonnxruntime.so
# Set by deploy.yml to the git SHA; reported in the x-clipos-version header on /healthz.
ARG CLIPOS_VERSION=dev
ENV CLIPOS_VERSION=${CLIPOS_VERSION}
USER 10001
EXPOSE 8081
ENTRYPOINT ["/usr/local/bin/clipos-worker"]

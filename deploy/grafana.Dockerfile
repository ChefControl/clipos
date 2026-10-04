# syntax=docker/dockerfile:1.7
# clipos Grafana: dashboards over Azure Monitor (platform metrics and the Log Analytics
# workspace the apps log to). Config, data source and dashboards are baked in; nothing is
# kept on disk, so a restart only signs people out (they're signed straight back in).
# Deployment settings (URL, Auth0 client, admins) are app settings: infra/azure/grafana.tf.

FROM grafana/grafana:13.2.3@sha256:b28bae15e219c998fb0e0424ed724930cc61b1f61fb404d47c862f9a23f9e572
COPY deploy/grafana/grafana.ini /etc/grafana/grafana.ini
COPY deploy/grafana/provisioning /etc/grafana/provisioning
COPY deploy/grafana/dashboards /etc/grafana/dashboards

# Served as /public/clipos-version.txt, where deploy.yml checks the rollout (the api and
# the worker report theirs on /healthz).
#
# The bundled data source plugins go: clipos only reads Azure Monitor, which is built into
# Grafana, and they're where the image's known vulnerabilities are (Trivy, audit.yml).
ARG CLIPOS_VERSION=dev
USER root
RUN rm -rf /usr/share/grafana/data/plugins-bundled/* \
    && echo "$CLIPOS_VERSION" > /usr/share/grafana/public/clipos-version.txt
USER grafana

# syntax=docker/dockerfile:1.7
# openweights-console — multi-stage build: Node 22 (pnpm via corepack) → nginx:alpine static serve.
# : Node 22 LTS + pnpm 9.x. : /health is a static file in public/ served as-is.

FROM node:22-alpine AS build
WORKDIR /app

# Enable pnpm via corepack; the package.json "packageManager" field pins the version.
RUN corepack enable

# Install deps first (better layer caching).
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

# Copy the rest of the source.
COPY . .

# Build-time args inject browser-visible origins (: non-secret).
ARG VITE_CAS_URL
ARG VITE_GATEWAY_URL
ARG VITE_CONFORMANCE_BADGE_URL
ENV VITE_CAS_URL=${VITE_CAS_URL}
ENV VITE_GATEWAY_URL=${VITE_GATEWAY_URL}
ENV VITE_CONFORMANCE_BADGE_URL=${VITE_CONFORMANCE_BADGE_URL}

RUN pnpm run build

# ----- Static serve stage -----
FROM nginx:1.27-alpine AS runtime

# Default nginx config serves /usr/share/nginx/html; that's where we put the SPA.
# SPA fallback for client routing: nginx's default try_files doesn't rewrite, so we
# ship a minimal override that routes unknown paths to index.html (except /health,
# which is a real file in public/).
RUN rm -f /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html

# Inline nginx config — SPA fallback + /health passthrough (the static file wins).
RUN printf '%s\n' \
  'server {' \
  '  listen 80 default_server;' \
  '  server_name _;' \
  '  root /usr/share/nginx/html;' \
  '  index index.html;' \
  '  location = /health { default_type application/json; add_header Cache-Control "no-store" always; try_files /health =404; }' \
  '  location / { try_files $uri $uri/ /index.html; }' \
  '}' > /etc/nginx/conf.d/default.conf

# Runtime config: nginx:alpine runs /docker-entrypoint.d/*.sh before starting.
# This regenerates /config.js from the operator's env on every container start,
# so a single prebuilt image can be pointed at any deployment.
COPY --chmod=0755 40-runtime-config.sh /docker-entrypoint.d/40-runtime-config.sh

EXPOSE 80
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
  CMD wget -q --spider http://127.0.0.1/health || exit 1

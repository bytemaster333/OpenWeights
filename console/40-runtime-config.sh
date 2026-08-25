#!/bin/sh
# runs from nginx:alpine's /docker-entrypoint.d/ before the server starts.
# writes the deployment URLs into /config.js from the operator's env, so one
# prebuilt image serves any deployment. empty values let the app fall back to
# its build-time defaults.
set -e
cat > /usr/share/nginx/html/config.js <<EOF
window.__OPENWEIGHTS_CONFIG__ = {
  CAS_URL: "${OPENWEIGHTS_CONSOLE_CAS_URL:-}",
  GATEWAY_URL: "${OPENWEIGHTS_CONSOLE_GATEWAY_URL:-}",
  HF_PROXY_URL: "${OPENWEIGHTS_CONSOLE_HF_PROXY_URL:-}"
};
EOF

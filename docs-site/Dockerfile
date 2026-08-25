# syntax=docker/dockerfile:1.7
FROM node:22-alpine AS build
WORKDIR /app
COPY package.json package-lock.json* ./
RUN npm ci || npm install
COPY . .
RUN npm run build

FROM nginx:1.27-alpine AS runtime
RUN rm -f /etc/nginx/conf.d/default.conf
COPY --from=build /app/out /usr/share/nginx/html
RUN printf '%s\n' \
  'server {' \
  '  listen 80 default_server;' \
  '  server_name _;' \
  '  root /usr/share/nginx/html;' \
  '  index index.html;' \
  '  absolute_redirect off;' \
  '  location / {' \
  '    rewrite ^/(.+)/$ /$1 permanent;' \
  '    try_files $uri $uri.html $uri/index.html =404;' \
  '  }' \
  '  error_page 404 /404.html;' \
  '  location = /404.html { internal; }' \
  '}' > /etc/nginx/conf.d/default.conf

EXPOSE 80
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
  CMD wget -q --spider http://127.0.0.1/ || exit 1

// Runtime deployment config. In dev (`pnpm dev`) this default empty object is
// served as-is and the app falls back to build-time VITE_* env. In the Docker
// image, the container entrypoint overwrites this file at start with the
// operator's URLs, so one prebuilt image serves any deployment.
window.__OPENWEIGHTS_CONFIG__ = {}

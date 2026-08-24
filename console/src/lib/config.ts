// Deployment URLs, resolved at runtime so ONE prebuilt console image works for
// any deployment. A static SPA normally bakes VITE_* at build time, which would
// hard-code a single CAS URL into the published image. Instead the container
// entrypoint writes /config.js at start (from the operator's env) into
// `window.__OPENWEIGHTS_CONFIG__`, and we read that here.
//
// Resolution order:
//   1. runtime /config.js       — the prebuilt-image path (operator sets env)
//   2. build-time VITE_* env     — the clone-and-build / `pnpm dev` path
//   3. localhost defaults        — bare `pnpm dev` with no env

type RuntimeConfig = {
  CAS_URL?: string
  GATEWAY_URL?: string
  HF_PROXY_URL?: string
}

declare global {
  interface Window {
    __OPENWEIGHTS_CONFIG__?: RuntimeConfig
  }
}

function resolve(runtime: string | undefined, build: string | undefined, fallback: string): string {
  const r = runtime?.trim()
  if (r) return r
  return build || fallback
}

const rc: RuntimeConfig = (typeof window !== "undefined" && window.__OPENWEIGHTS_CONFIG__) || {}

export const CAS_URL = resolve(rc.CAS_URL, import.meta.env.VITE_CAS_URL, "http://localhost:8080")
export const GATEWAY_URL = resolve(
  rc.GATEWAY_URL,
  import.meta.env.VITE_GATEWAY_URL,
  "http://localhost:9090",
)
export const HF_PROXY_URL = resolve(
  rc.HF_PROXY_URL,
  import.meta.env.VITE_HF_PROXY_URL,
  "http://localhost:28090",
)

/**
 * Fetch wrappers for the CAS admin API and the gateway.
 *
 * Design notes:
 * - `credentials: "include"` on every call — the session cookie (`siahub_session`,
 * ) is `HttpOnly; SameSite=Lax; Secure` and set by CAS on OAuth callback.
 * - 403 vs 401 discipline (see / 04-CONTEXT §4.7):
 * - 401 — session expired; hooks should route the user to `/login`.
 * - 403 — signed-URL or admin-scope issue; hooks should surface a toast with
 * the "Refresh URL" semantics rather than logging the user out.
 * This module surfaces the raw status via `ApiError`; per-hook code decides.
 * - No auth header is attached. Plaintext API keys live only in the
 * `POST /admin/keys` response body + onboarding copy-paste.*/

const CAS_URL = import.meta.env.VITE_CAS_URL ?? "http://localhost:8080"
const GATEWAY_URL = import.meta.env.VITE_GATEWAY_URL ?? "http://localhost:9090"

export class ApiError extends Error {
  readonly status: number
  readonly code: string
  readonly requestId?: string

  constructor(status: number, code: string, requestId?: string) {
    super(`${status} ${code}`)
    this.status = status
    this.code = code
    this.requestId = requestId
  }
}

async function request<T>(base: string, path: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(`${base}${path}`, {
    credentials: "include",
    headers: { Accept: "application/json", ...init.headers },
    ...init,
  })
  const requestId = res.headers.get("x-request-id") ?? undefined
  if (!res.ok) {
    let code = res.statusText
    try {
      const body = (await res.json()) as { code?: string }
      if (body?.code) code = body.code
    } catch {
      // Response body not JSON — fall through with the statusText.
    }
    throw new ApiError(res.status, code, requestId)
  }
  // 204 No Content — return undefined as T.
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

export function casFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  return request<T>(CAS_URL, path, init)
}

export function gatewayFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  return request<T>(GATEWAY_URL, path, init)
}

/**
 * Log the current user out. Hits CAS `POST /auth/logout`, which marks the
 * session `revoked_at = NOW` and clears the `siahub_session` cookie via
 * `Set-Cookie: Max-Age=0` ( 04-CONTEXT §4.6). Callers should clear
 * any cached user data after this resolves.*/
export function logout(): Promise<void> {
  return casFetch<void>("/auth/logout", { method: "POST" })
}

export { CAS_URL, GATEWAY_URL }

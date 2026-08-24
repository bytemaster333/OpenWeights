/**
 * Fetch wrappers for the CAS admin API and the gateway.
 *
 * Design notes:
 * - `credentials: "include"` on every call — the session cookie (`openweights_session`,
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
  // Destructure `headers` out and apply it last so per-call headers always
  // merge with the defaults instead of clobbering them.
  const { headers: initHeaders, ...rest } = init
  const res = await fetch(`${base}${path}`, {
    credentials: "include",
    ...rest,
    headers: { Accept: "application/json", ...initHeaders },
  })
  const requestId = res.headers.get("x-request-id") ?? undefined
  if (!res.ok) {
    let code = res.statusText
    try {
      // Backend AppError emits `{"error": "..."}`; the OAuth callback path
      // emits `{"code": "..."}`. Read either to surface the semantic
      // string instead of a generic "Bad Request"/"Unauthorized".
      const body = (await res.json()) as { code?: string; error?: string }
      const semantic = body?.code ?? body?.error
      if (semantic) code = semantic
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
 * session `revoked_at = NOW` and clears the `openweights_session` cookie via
 * `Set-Cookie: Max-Age=0` ( 04-CONTEXT §4.6). Callers should clear
 * any cached user data after this resolves.*/
export function logout(): Promise<void> {
  return casFetch<void>("/auth/logout", { method: "POST" })
}

/** Which sign-in methods the operator enabled. Drives the login page UI. */
export interface AuthMethods {
  password: boolean
  github: boolean
}

/**
 * Public probe of the enabled sign-in methods. Lets the login page show a
 * password box, a GitHub button, or both, without baking the choice into the
 * build. Never returns secrets — only two booleans.
 */
export function authMethods(): Promise<AuthMethods> {
  return casFetch<AuthMethods>("/auth/methods")
}

/**
 * Password sign-in for single-operator self-hosting. Hits CAS
 * `POST /auth/login`, which on success sets the same `openweights_session`
 * cookie the GitHub flow would. Throws `ApiError` (401) on a wrong password.
 */
export function login(password: string): Promise<void> {
  return casFetch<void>("/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ password }),
  })
}

export { CAS_URL, GATEWAY_URL }

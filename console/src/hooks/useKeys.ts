import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"

import { type ApiError, casFetch } from "@/lib/api"

/**
 * API-key scopes understood by CAS `/admin/keys` (see Task 4).
 *
 * - `read` — download via gateway signed URLs only.
 * - `write` — full Xet protocol (xorb upload, shard upload, reconstruction).
 * - `admin` — issued by `/admin/setup` only; out of scope for onboarding.*/
export type KeyScope = "read" | "write" | "admin"

/**
 * Shape returned by `POST /admin/keys`. `plaintext_key` is only present on
 * creation — `GET /admin/keys` returns the masked prefix only. See :
 * the plaintext must live **only** in React component state bounded by the
 * one-time modal; it MUST NOT be written to `localStorage`, `sessionStorage`,
 * cookies, analytics, telemetry, or any logging sink.*/
export type CreatedKey = {
  id: string
  name: string
  scope: KeyScope
  masked_prefix: string
  plaintext_key: string
  created_at: string
}

/**
 * Body sent to `POST /admin/keys`.*/
export type CreateKeyBody = {
  name: string
  scope: KeyScope
}

/**
 * TanStack mutation for creating a fresh API key.
 *
 * Invalidates the `["keys"]` query key on success so that 04-05's key-list
 * page picks up the new row automatically. Does NOT persist the plaintext
 * anywhere — the caller component owns the lifecycle and is expected to
 * mount the `<OneTimeKeyModal>` with the returned `plaintext_key`.*/
export function useCreateKey() {
  const qc = useQueryClient()
  return useMutation<CreatedKey, ApiError, CreateKeyBody>({
    mutationFn: (body) =>
      casFetch<CreatedKey>("/admin/keys", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["keys"] })
    },
  })
}

/**
 * Shape returned by `GET /admin/keys`. CAS SELECTs
 * `id, name, scope, masked_prefix, created_at, last_used_at`
 * from `api_keys` WHERE `user_id=$session_user AND revoked_at IS NULL`.
 *
 * Load-bearing invariant ( plan Task 1): this shape MUST NOT
 * include `plaintext_key`. The plaintext is issued exactly once on
 * `POST /admin/keys` and is never re-returned by any list endpoint. The
 * console must never attempt to surface it from the table.*/
export type ListedKey = {
  id: string
  name: string
  scope: KeyScope
  masked_prefix: string
  created_at: string
  last_used_at: string | null
}

/**
 * TanStack query for the authenticated user's active API keys. Paired with
 * `useCreateKey` (invalidates on mutate success) and `useRevokeKey` (same).
 *
 * `staleTime: 10_000` matches 's stats cadence — the keys list is
 * intentionally not hot-polled. Server-side, last-used-at refresh is
 * capped at 5s debounce ( task 8), so a 10s stale window never shows
 * older data than CAS has.*/
export function useKeys() {
  return useQuery<ListedKey[], ApiError>({
    queryKey: ["keys"],
    queryFn: () => casFetch<{ keys: ListedKey[] }>("/admin/keys").then((r) => r.keys),
    staleTime: 10_000,
  })
}

/**
 * TanStack mutation for `DELETE /admin/keys/{id}`. On success the `["keys"]`
 * query is invalidated so the table removes the row.
 *
 * SLO (5s): enforced server-side (Redis cache invalidation + auth
 * middleware check on `revoked_at`). The client just has to trust the DELETE
 * acknowledgement. The confirmation `AlertDialog` is UX, not a safety gate.*/
export function useRevokeKey() {
  const qc = useQueryClient()
  return useMutation<void, ApiError, string>({
    mutationFn: (id) => casFetch<void>(`/admin/keys/${id}`, { method: "DELETE" }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["keys"] })
    },
  })
}

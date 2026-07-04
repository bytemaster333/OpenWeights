import { useQuery } from "@tanstack/react-query"

import { type ApiError, casFetch } from "@/lib/api"

/**
 * A single row in the `GET /admin/xorbs` response, as returned by
 * `cas/crates/openweights-cas-core/src/handlers/admin/xorbs.rs` (XorbRow).
 *
 * Field-by-field notes:
 * - `hash` — lowercase hex of `xorbs.xorb_merkle_hash` (full 64 chars). The
 * CAS already does the byte-reversal-per-8-byte-group dance on write
 * so this string is the canonical display form.
 * - `sia_object_id` — `NULL` until gateway writes the xorb to Sia
 * and `indexd` returns an object id. `pin_state="pinning"` implies NULL;
 * `pin_state="pinned"` implies a non-null value.
 * - `size_bytes` — `i64` on the wire, safe for Number in JS up to 2^53-1.
 * - `pin_state` — union of the `xorb_pin_state` Postgres enum values.
 * - `uploaded_at` — ISO-8601 UTC (serde `DateTime<Utc>` → RFC3339).
 * - `uploader_key_id` — UUID string; NOT NULL in schema (every xorb has an
 * owning key). Different from 04-06-PLAN's `string | null` typing.*/
export type Xorb = {
  hash: string
  sia_object_id: string | null
  size_bytes: number
  /**
   * Real xorbs go through the Sia reconciler (uploading → pinning → pinned,
   * orphaned on retry-budget exhaustion). The synthetic 'inline' state is
   * only emitted by the admin handler's UNION over `lfs_objects`: small
   * legacy LFS files are stored as Postgres BYTEA and never reach Sia, so
   * they're labeled distinctly to avoid the "Pinned on Sia" + "no Sia
   * object id" contradiction.
   */
  pin_state: "uploading" | "pinning" | "pinned" | "orphaned" | "inline"
  uploaded_at: string
  uploader_key_id: string
}

/**
 * Query-time filter set consumed by `useAssets`.
 *
 * - `hashPrefix` — the raw, user-typed string from the search box. The hook
 * only forwards it to CAS when it is exactly 8 lowercase hex chars; any
 * other value is dropped server-side (CAS returns 400 `BadRequest` for
 * non-8-hex prefixes, so sending partial input would break the list).
 * Shorter prefixes trigger client-side `startsWith` filtering instead
 * (see `useAssets` body).
 * - `apiKeyId` — UUID of an API key to narrow the list to. `null` disables
 * the filter; an empty string is treated as `null` for URL-binding
 * convenience.*/
export type AssetFilters = {
  hashPrefix?: string
  apiKeyId?: string | null
}

type ListXorbsResponse = { xorbs: Xorb[] }

const HEX_8 = /^[0-9a-f]{8}$/

/**
 * Core TanStack query for `GET /admin/xorbs`. Returns up to 500 rows per
 * the CAS handler's hard `LIMIT 500` (pagination past the first page is
 * deferred to per 04-06-PLAN).
 *
 * Server-side filtering (`?hash_prefix=&api_key_id=`) is used when the
 * input is valid for CAS; otherwise the hook falls back to client-side
 * filtering so that the user sees responsive feedback as they type. The
 * returned list is always the filtered view.*/
export function useAssets(filters: AssetFilters) {
  const rawPrefix = (filters.hashPrefix ?? "").trim().toLowerCase()
  const apiKeyId = filters.apiKeyId?.trim() ? filters.apiKeyId.trim() : null

  // Only forward prefix to CAS when exactly 8 hex chars — anything else
  // would 400. Partial input still triggers client-side narrowing below.
  const serverPrefix = HEX_8.test(rawPrefix) ? rawPrefix : undefined

  return useQuery<Xorb[], ApiError>({
    queryKey: ["assets", { serverPrefix, apiKeyId, clientPrefix: rawPrefix }],
    queryFn: async () => {
      const params = new URLSearchParams()
      if (serverPrefix) params.set("hash_prefix", serverPrefix)
      if (apiKeyId) params.set("api_key_id", apiKeyId)
      const q = params.toString()
      const res = await casFetch<ListXorbsResponse>(`/admin/xorbs${q ? `?${q}` : ""}`)

      // Client-side narrowing for sub-8-char prefixes. No-op when the server
      // already applied a full 8-char filter (rawPrefix === serverPrefix in
      // that case; `startsWith` is trivially true).
      if (rawPrefix && !serverPrefix) {
        return res.xorbs.filter((x) => x.hash.toLowerCase().startsWith(rawPrefix))
      }
      return res.xorbs
    },
    staleTime: 15_000,
  })
}

/**
 * Detail payload for a single xorb, as returned (in the future) by
 * `GET /admin/xorbs/{hash}`.
 *
 * **Cross-plan dependency (recorded in SUMMARY):** CAS did not
 * ship `/admin/xorbs/{hash}` — only the list endpoint. Until a follow-up
 * CAS plan adds the detail route, `useAsset` falls back to the list query
 * cache (filtered by full hash) and returns `referencing_repos: []`.*/
export type AssetDetail = {
  xorb: Xorb
  referencing_repos: string[]
}

/**
 * Fetch detail for a single xorb hash. See `AssetDetail` for the
 * fallback story while CAS does not expose `/admin/xorbs/{hash}`.
 *
 * Strategy:
 * 1. Try `GET /admin/xorbs/{hash}`. If it 200s, use the payload.
 * 2. On 404 (route not implemented or hash unknown), fall back to a
 * full-list scan with `hash_prefix` = first 8 chars of hash. If a row
 * matches the full hash, synthesize `{xorb, referencing_repos: []}`.
 * 3. If neither yields a row, throw the original `ApiError` so the page
 * can render "Not found".*/
export function useAsset(hash: string) {
  return useQuery<AssetDetail, ApiError>({
    queryKey: ["asset", hash],
    enabled: !!hash,
    queryFn: async () => {
      try {
        return await casFetch<AssetDetail>(`/admin/xorbs/${hash}`)
      } catch (e) {
        if (!(e instanceof Error) || !("status" in e)) throw e
        const err = e as ApiError
        // Route not wired yet, or hash truly unknown — fall back to list scan.
        if (err.status !== 404 && err.status !== 405) throw err

        // Only 8-hex prefixes are valid server-side. A hash shorter than 8
        // hex chars can never match a real xorb, so skip the scan.
        const prefix = hash.slice(0, 8).toLowerCase()
        if (!HEX_8.test(prefix)) throw err

        const { xorbs } = await casFetch<ListXorbsResponse>(`/admin/xorbs?hash_prefix=${prefix}`)
        const match = xorbs.find((x) => x.hash.toLowerCase() === hash.toLowerCase())
        if (!match) throw err
        return { xorb: match, referencing_repos: [] }
      }
    },
  })
}

import { useQuery } from "@tanstack/react-query"

import { type ApiError, casFetch } from "@/lib/api"

/**
 * One row in `GET /api/models`. Matches `ModelListItem` in
 * `cas/crates/siahub-cas-core/src/handlers/hfapi.rs`.
 *
 * - `id`: canonical `"<owner>/<name>"` (HF-compat shape).
 * - `author`: owner's GitHub login (or `hf:<userId>` for JWT-auth users).
 * - `lastModified`: RFC 3339 of `repos.updated_at`.
 * - `size`: sum of `repo_files.size_bytes` at `main` HEAD.
 * - `files`: count of `repo_files` rows at `main` HEAD.*/
export type ModelListItem = {
  id: string
  author: string
  name: string
  description: string | null
  lastModified: string
  size: number
  files: number
  /** All-time download count from `repo_downloads`.*/
  downloadsTotal: number
  /** Rolling 30-day download count.*/
  downloads30d: number
}

/**
 * `GET /api/models` — anonymous-readable list of public repos. No query
 * filter in V1; the response is already capped at 200 rows server-side.*/
export function useModels() {
  return useQuery<ModelListItem[], ApiError>({
    queryKey: ["models"],
    queryFn: () => casFetch<ModelListItem[]>("/api/models"),
    staleTime: 30_000,
  })
}

/**
 * Sibling file in a model revision. Mirrors `ModelSibling` in hfapi.rs:
 * `blob_id` is the content hash (xet-hash or lfs-oid in hex), `lfs` is
 * populated for every Xet/LFS file so the `<Files>` table can render
 * per-row download links through `/{owner}/{repo}/resolve/main/{path}`.*/
export type ModelSibling = {
  rfilename: string
  size: number
  blob_id: string | null
  lfs: {
    oid: string
    size: number
    pointer_size?: number | null
  } | null
}

export type ModelInfo = {
  id: string
  author: string
  sha: string
  lastModified: string
  private: boolean
  siblings: ModelSibling[]
  createdAt: string
  cardData?: unknown
  /** Rolling download counters populated from `repo_downloads`.*/
  downloads: {
    total: number
    last_24h: number
    last_7d: number
    last_30d: number
  }
}

/**
 * Zero-filled 14-day download trend for the model detail sparkline.
 * Backend: `GET /api/models/:owner/:repo/downloads/trend`.*/
export type ModelTrendPoint = {
  day: string
  count: number
  bytes: number
}

export function useModelTrend(
  owner: string | undefined,
  repo: string | undefined,
) {
  return useQuery<ModelTrendPoint[], ApiError>({
    queryKey: ["models", owner, repo, "trend"],
    queryFn: () =>
      casFetch<{ days: ModelTrendPoint[] }>(
        `/api/models/${owner}/${repo}/downloads/trend`,
      ).then((r) => r.days),
    enabled: Boolean(owner && repo),
    staleTime: 30_000,
  })
}

export function useModelInfo(owner: string | undefined, repo: string | undefined) {
  return useQuery<ModelInfo, ApiError>({
    queryKey: ["models", owner, repo],
    queryFn: () => casFetch<ModelInfo>(`/api/models/${owner}/${repo}`),
    enabled: Boolean(owner && repo),
    staleTime: 10_000,
  })
}

/**
 * Underlying objects (xorbs + inline LFS blobs) referenced by a model's
 * HEAD commit. Used by the ModelDetail "Objects on Sia" panel so reviewers
 * can deep-link from a model down to the actual content-addressed blobs
 * (and verify their pin state on Sia).
 */
export type ModelObject = {
  hash: string
  /** "xorb" for Xet xorbs (go through Sia), "inline" for small LFS blobs
   * (Postgres BYTEA, never reach Sia).*/
  kind: "xorb" | "inline"
  size_bytes: number
  pin_state: "uploading" | "pinning" | "pinned" | "orphaned" | "inline"
  sia_object_id: string | null
  /** Repo-relative file paths sharing this xorb (xet packs multi-file). */
  files: string[]
}

export function useModelObjects(
  owner: string | undefined,
  repo: string | undefined,
) {
  return useQuery<ModelObject[], ApiError>({
    queryKey: ["models", owner, repo, "objects"],
    queryFn: () =>
      casFetch<{ objects: ModelObject[] }>(
        `/api/models/${owner}/${repo}/objects`,
      ).then((r) => r.objects),
    enabled: Boolean(owner && repo),
    staleTime: 10_000,
  })
}

/**
 * Fetch the raw README.md bytes for the given repo's main HEAD via the
 * LFS-resolve redirect path. Returns `null` when the repo has no README.
 * The fetch follows redirects (default), so we end up reading from
 * `/lfs/objects/{oid}`.*/
export function useModelReadme(owner: string | undefined, repo: string | undefined) {
  return useQuery<string | null, Error>({
    queryKey: ["models", owner, repo, "README.md"],
    queryFn: async () => {
      if (!owner || !repo) return null
      const base = import.meta.env.VITE_CAS_URL ?? "http://localhost:8080"
      const res = await fetch(`${base}/${owner}/${repo}/resolve/main/README.md`, {
        redirect: "follow",
      })
      if (res.status === 404) return null
      if (!res.ok) throw new Error(`README fetch failed: ${res.status}`)
      return await res.text()
    },
    enabled: Boolean(owner && repo),
    staleTime: 60_000,
  })
}

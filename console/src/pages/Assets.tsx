import { CheckIcon, CopyIcon, MagnifyingGlassIcon } from "@phosphor-icons/react"
import { Link } from "@tanstack/react-router"
import { useEffect, useMemo, useState } from "react"

import { UserMenu } from "@/components/UserMenu"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useAssets } from "@/hooks/useAssets"
import { useKeys } from "@/hooks/useKeys"
import { useMe } from "@/hooks/useMe"

/**
 * `/assets` page (CONSOLE-03, CONSOLE-04, CONSOLE-05).
 *
 * Admin-scoped xorb catalog. Operators use this page for:
 *
 *   1. Auditing what's stored — "is this 64 MiB spike from key `sia_live_c0`
 *      or `sia_live_7e`?" (04-CONTEXT §4.6 demo scenario).
 *   2. Troubleshooting stuck uploads — rows with `pin_state="pinning"` that
 *      never transition to `pinned` are the canonical Phase 3 bug signal.
 *   3. Verifying the "bytes are on Sia" claim — each row's `sia_object_id`
 *      is a click away from `indexd` and, ultimately, from proof that the
 *      xorb rides on Sia hosts rather than centralized storage.
 *
 * URL binding (CONSOLE-04/-05):
 *   - `?hash_prefix=<1..64 hex>` — free-form hex fragment. 8-char inputs
 *     go to CAS as `hash_prefix`; everything else is client-side narrowing.
 *   - `?api_key_id=<uuid>` — UUID of the owning key. Missing/empty = all.
 *
 * Search input is debounced through a 250ms timer so that a user typing
 * "deadbeef" doesn't fire 8 CAS requests; only the settled value flips to
 * the query-key + the URL.
 */

const SEARCH_DEBOUNCE_MS = 250
const HASH_TRUNC = 16
const KEY_ID_TRUNC = 8

function formatBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"]
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i += 1
  }
  return `${v.toFixed(v < 10 && i > 0 ? 2 : 0)} ${units[i]}`
}

function pinStateVariant(s: string): "default" | "secondary" | "destructive" | "outline" {
  switch (s) {
    case "pinned":
      return "default"
    case "orphaned":
      return "destructive"
    case "pinning":
    case "uploading":
      return "secondary"
    default:
      return "outline"
  }
}

/** Small copy-hash button. Uses the same "Copied" flash pattern as the
 * onboarding env-block card but trimmed for inline table use. */
function CopyHashButton({ hash }: { hash: string }) {
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), 1500)
    return () => clearTimeout(t)
  }, [copied])
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      aria-label={copied ? "Hash copied" : "Copy full hash"}
      data-testid={`asset-copy-${hash.slice(0, 8)}`}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        navigator.clipboard
          .writeText(hash)
          .then(() => setCopied(true))
          .catch(() => {
            // Clipboard denied — swallow; user can still select manually.
          })
      }}
    >
      {copied ? <CheckIcon data-icon="inline-start" /> : <CopyIcon data-icon="inline-start" />}
      <span className="sr-only">Copy hash</span>
    </Button>
  )
}

export function AssetsPage() {
  const { data: user } = useMe()

  // Live search value (what the user is typing), plus the debounced value
  // that actually flows into `useAssets` + the URL (on settle).
  const [hashPrefix, setHashPrefix] = useState("")
  const [debouncedPrefix, setDebouncedPrefix] = useState("")
  const [apiKeyId, setApiKeyId] = useState<string | null>(null)

  // Initialize filter state from URL on first mount, then keep URL in sync.
  // `useSearch` from TanStack Router is type-strict; we read raw
  // `window.location.search` so we don't have to wire `validateSearch`
  // into the route (keeps `router.tsx` deviation-free — see 04-CONTEXT §6).
  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const hp = params.get("hash_prefix") ?? ""
    const kid = params.get("api_key_id")
    if (hp) {
      setHashPrefix(hp)
      setDebouncedPrefix(hp)
    }
    if (kid) setApiKeyId(kid)
    // Only on mount — downstream URL changes are driven by state, not the
    // other way around. If the user clicks a deep link, the page remounts.
  }, [])

  // Debounce the search input. The effect runs on every keystroke but only
  // the last-set timer wins.
  useEffect(() => {
    const t = setTimeout(() => setDebouncedPrefix(hashPrefix.trim()), SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [hashPrefix])

  // Reflect the settled filter state back to the URL so that reloads +
  // shareable links preserve the view (CONSOLE-04/-05 spec).
  useEffect(() => {
    const params = new URLSearchParams()
    if (debouncedPrefix) params.set("hash_prefix", debouncedPrefix)
    if (apiKeyId) params.set("api_key_id", apiKeyId)
    const q = params.toString()
    const target = `${window.location.pathname}${q ? `?${q}` : ""}`
    // Only push when actually different — avoids polluting history with
    // duplicate entries under StrictMode double-invoke.
    if (target !== `${window.location.pathname}${window.location.search}`) {
      window.history.replaceState({}, "", target)
    }
  }, [debouncedPrefix, apiKeyId])

  const { data: keys = [] } = useKeys()
  const {
    data: xorbs = [],
    isPending,
    error,
  } = useAssets({ hashPrefix: debouncedPrefix, apiKeyId })

  // Lookup table so the table cell can render the key's friendly
  // `masked_prefix` instead of the raw UUID.
  const keyLabel = useMemo(() => {
    const m = new Map<string, string>()
    for (const k of keys) m.set(k.id, k.masked_prefix)
    return m
  }, [keys])

  return (
    <main className="mx-auto max-w-5xl px-6 py-10">
      <header className="mb-8 flex items-center justify-between">
        <h1 className="font-heading text-2xl font-medium tracking-tight">Assets</h1>
        {user ? <UserMenu user={user} /> : null}
      </header>

      <div className="mb-6 flex flex-wrap items-center gap-3">
        <div className="relative max-w-sm flex-1">
          <MagnifyingGlassIcon
            className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input
            placeholder="Search by hash prefix…"
            value={hashPrefix}
            onChange={(e) => setHashPrefix(e.target.value)}
            className="pl-9"
            data-testid="assets-search"
            aria-label="Search assets by hash prefix"
          />
        </div>

        <Select
          value={apiKeyId ?? "all"}
          onValueChange={(v) => setApiKeyId(v === "all" ? null : v)}
        >
          <SelectTrigger className="w-56" data-testid="assets-key-filter">
            <SelectValue placeholder="All keys" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All keys</SelectItem>
            {keys.map((k) => (
              <SelectItem key={k.id} value={k.id}>
                {k.name ?? "unnamed"} ({k.masked_prefix})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <Table data-testid="assets-table">
        <TableHeader>
          <TableRow>
            <TableHead className="w-[34%]">Hash</TableHead>
            <TableHead>Size</TableHead>
            <TableHead>Pin state</TableHead>
            <TableHead>Uploaded</TableHead>
            <TableHead>Key</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {isPending &&
            Array.from({ length: 5 }).map((_, i) => (
              <TableRow key={`skeleton-${i.toString()}`}>
                <TableCell colSpan={5}>
                  <Skeleton className="h-5 w-full" />
                </TableCell>
              </TableRow>
            ))}
          {error && !isPending && (
            <TableRow>
              <TableCell colSpan={5} className="text-destructive" data-testid="assets-error">
                Failed to load assets: {error.message}
              </TableCell>
            </TableRow>
          )}
          {!isPending && !error && xorbs.length === 0 && (
            <TableRow>
              <TableCell colSpan={5} className="text-muted-foreground" data-testid="assets-empty">
                No xorbs match the current filter.
              </TableCell>
            </TableRow>
          )}
          {!isPending &&
            !error &&
            xorbs.map((x) => (
              <TableRow key={x.hash}>
                <TableCell className="font-mono text-sm">
                  <div className="flex items-center gap-2">
                    <Link
                      to="/assets/$hash"
                      params={{ hash: x.hash }}
                      className="underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-foreground"
                      data-testid={`asset-link-${x.hash.slice(0, 8)}`}
                    >
                      {x.hash.slice(0, HASH_TRUNC)}…
                    </Link>
                    <CopyHashButton hash={x.hash} />
                  </div>
                </TableCell>
                <TableCell>{formatBytes(x.size_bytes)}</TableCell>
                <TableCell>
                  <Badge variant={pinStateVariant(x.pin_state)}>{x.pin_state}</Badge>
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {new Date(x.uploaded_at).toLocaleString()}
                </TableCell>
                <TableCell className="font-mono text-xs text-muted-foreground">
                  {keyLabel.get(x.uploader_key_id) ??
                    `${x.uploader_key_id.slice(0, KEY_ID_TRUNC)}…`}
                </TableCell>
              </TableRow>
            ))}
        </TableBody>
      </Table>

      {!isPending && !error && xorbs.length >= 500 && (
        <p className="mt-4 text-xs text-muted-foreground" data-testid="assets-pagination-note">
          Showing the first 500 rows. Narrow with search or the key filter; pagination beyond the
          first page is deferred to Phase 5.
        </p>
      )}
    </main>
  )
}

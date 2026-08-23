import {
  ArrowSquareOutIcon,
  CheckIcon,
  CircleNotchIcon,
  CloudCheckIcon,
  CloudSlashIcon,
  CopyIcon,
  DatabaseIcon,
  MagnifyingGlassIcon,
  WarningIcon,
} from "@phosphor-icons/react"
import { Link } from "@tanstack/react-router"
import { useEffect, useMemo, useState } from "react"

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
import { type Xorb, useAssets } from "@/hooks/useAssets"
import { useKeys } from "@/hooks/useKeys"
import { usePlatformSia } from "@/hooks/useSetupStatus"
import { formatBytes, formatRelative } from "@/lib/format"

/**
 * `/assets` — operator-facing xorb catalog.
 *
 * Revamped layout:
 *
 * 1. Summary strip (total count + bytes + per-pin-state breakdown).
 * 2. Filter row (hash prefix + API key).
 * 3. Rows = {hash, size, pin state, uploaded, key}. Pin state carries
 * a descriptive label + icon so the `pinning` / `orphaned` cases
 * explain themselves without the user opening the detail page.*/

const SEARCH_DEBOUNCE_MS = 250
const HASH_TRUNC = 16
const KEY_ID_TRUNC = 8
/** Sentinel uploader_key_id emitted by the admin handler when an inline LFS
 * row exists with no api_keys binding (orphan upload pre-commit). */
const ZERO_UUID = "00000000-0000-0000-0000-000000000000"

type PinMeta = {
  label: string
  icon: React.ComponentType<{
    size?: number
    weight?: "regular" | "bold" | "fill" | "light" | "thin" | "duotone"
  }>
  variant: "default" | "secondary" | "destructive" | "outline"
  hint: string
}

function pinMeta(s: string): PinMeta {
  switch (s) {
    case "pinned":
      return {
        label: "Pinned on Sia",
        icon: CloudCheckIcon,
        variant: "default",
        hint: "Bytes acknowledged by at least one Sia storage host.",
      }
    case "pinning":
      return {
        label: "Sia-pending",
        icon: CircleNotchIcon,
        variant: "secondary",
        hint: "Uploaded to CAS; waiting for Sia host contracts. Reconciler retries automatically.",
      }
    case "uploading":
      return {
        label: "Uploading",
        icon: CircleNotchIcon,
        variant: "secondary",
        hint: "Bytes are still arriving at the CAS.",
      }
    case "orphaned":
      return {
        label: "Orphaned",
        icon: CloudSlashIcon,
        variant: "destructive",
        hint: "Reconciler exhausted its retry budget. Manual re-drive needed once Sia hosts are available.",
      }
    case "inline":
      return {
        label: "Stored inline",
        icon: DatabaseIcon,
        variant: "outline",
        hint: "Small LFS file kept as a Postgres BYTEA blob; never goes to Sia.",
      }
    default:
      return {
        label: s,
        icon: WarningIcon,
        variant: "outline",
        hint: "Unknown pin state.",
      }
  }
}

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
          .catch(() => {})
      }}
    >
      {copied ? <CheckIcon size={12} /> : <CopyIcon size={12} />}
      <span className="sr-only">Copy hash</span>
    </Button>
  )
}

// ---------------------------------------------------------------------------
// Summary strip
// ---------------------------------------------------------------------------

function SummaryStrip({ xorbs }: { xorbs: Xorb[] }) {
  const totalBytes = xorbs.reduce((n, x) => n + x.size_bytes, 0)
  const byState = useMemo(() => {
    const m: Record<string, number> = {}
    for (const x of xorbs) m[x.pin_state] = (m[x.pin_state] ?? 0) + 1
    return m
  }, [xorbs])

  const cards = [
    { label: "Xorbs", value: xorbs.length.toString(), hint: "total" },
    { label: "Bytes", value: formatBytes(totalBytes), hint: "uploaded" },
    {
      label: "Sia-pinned",
      value: (byState.pinned ?? 0).toString(),
      hint: "on hosts",
    },
    {
      label: "Pending / orphaned",
      value: ((byState.pinning ?? 0) + (byState.orphaned ?? 0)).toString(),
      hint: "awaiting hosts",
    },
  ]

  return (
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
      {cards.map((c) => (
        <div key={c.label} className="rounded border bg-muted/10 p-4">
          <div className="text-xs uppercase tracking-wide text-muted-foreground">{c.label}</div>
          <div className="mt-1 text-2xl font-semibold">{c.value}</div>
          <div className="mt-0.5 text-xs text-muted-foreground">{c.hint}</div>
        </div>
      ))}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export function AssetsPage() {
  const [hashPrefix, setHashPrefix] = useState("")
  const [debouncedPrefix, setDebouncedPrefix] = useState("")
  const [apiKeyId, setApiKeyId] = useState<string | null>(null)

  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const hp = params.get("hash_prefix") ?? ""
    const kid = params.get("api_key_id")
    if (hp) {
      setHashPrefix(hp)
      setDebouncedPrefix(hp)
    }
    if (kid) setApiKeyId(kid)
  }, [])

  useEffect(() => {
    const t = setTimeout(() => setDebouncedPrefix(hashPrefix.trim()), SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [hashPrefix])

  useEffect(() => {
    const params = new URLSearchParams()
    if (debouncedPrefix) params.set("hash_prefix", debouncedPrefix)
    if (apiKeyId) params.set("api_key_id", apiKeyId)
    const q = params.toString()
    const target = `${window.location.pathname}${q ? `?${q}` : ""}`
    if (target !== `${window.location.pathname}${window.location.search}`) {
      window.history.replaceState({}, "", target)
    }
  }, [debouncedPrefix, apiKeyId])

  const { data: keys = [] } = useKeys()
  const { data: sia } = usePlatformSia()
  const {
    data: xorbs = [],
    isPending,
    error,
  } = useAssets({ hashPrefix: debouncedPrefix, apiKeyId })

  const keyLabel = useMemo(() => {
    const m = new Map<string, string>()
    for (const k of keys) m.set(k.id, k.masked_prefix)
    return m
  }, [keys])

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 py-8">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <div className="flex items-center gap-3">
            <DatabaseIcon size={22} weight="light" className="text-muted-foreground" />
            <h1 className="font-heading text-2xl font-semibold tracking-tight">Assets</h1>
          </div>
          <p className="mt-1 text-sm text-muted-foreground">
            Every xorb on this deployment. Click a hash to see its repo.
          </p>
        </div>
        {sia && (
          <a
            href={
              sia.wallet_address
                ? `${sia.siascan_base}/address/${sia.wallet_address}`
                : sia.siascan_base
            }
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-2 rounded border bg-muted/10 px-3 py-1.5 text-xs hover:border-primary/40 hover:bg-muted/30"
            title={
              sia.wallet_address
                ? `View renter wallet on ${sia.siascan_base.replace(/^https?:\/\//, "")}`
                : "View Sia explorer"
            }
          >
            <CloudCheckIcon size={14} weight="light" className="text-primary" />
            <span>
              <span className="font-mono">{sia.contract_count}</span> contracts ·{" "}
              <span className="font-mono">{sia.distinct_host_count}</span> hosts on{" "}
              {sia.siascan_base.includes("zen") ? "Zen testnet" : "Sia"}
            </span>
            <ArrowSquareOutIcon size={12} weight="light" />
          </a>
        )}
      </header>

      <SummaryStrip xorbs={xorbs} />

      <div className="flex flex-wrap items-center gap-3">
        <div className="relative max-w-sm flex-1">
          <MagnifyingGlassIcon
            size={16}
            className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground"
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

      <div className="rounded border">
        <Table data-testid="assets-table">
          <TableHeader>
            <TableRow>
              <TableHead className="w-[30%]">Hash</TableHead>
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
                <TableCell
                  colSpan={5}
                  className="py-10 text-center text-muted-foreground"
                  data-testid="assets-empty"
                >
                  No xorbs match the current filter.
                </TableCell>
              </TableRow>
            )}
            {!isPending &&
              !error &&
              xorbs.map((x) => {
                const pm = pinMeta(x.pin_state)
                const Icon = pm.icon
                return (
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
                      <span className="inline-flex items-center gap-1.5" title={pm.hint}>
                        <Icon size={14} weight="light" />
                        <Badge variant={pm.variant}>{pm.label}</Badge>
                        {x.pin_state === "pinned" && sia?.wallet_address && (
                          <a
                            href={`${sia.siascan_base}/address/${sia.wallet_address}`}
                            target="_blank"
                            rel="noreferrer"
                            className="text-muted-foreground hover:text-primary"
                            title="View renter wallet on siascan"
                            onClick={(e) => e.stopPropagation()}
                          >
                            <ArrowSquareOutIcon size={12} weight="light" />
                          </a>
                        )}
                      </span>
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {formatRelative(x.uploaded_at)}
                    </TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      {x.uploader_key_id === ZERO_UUID
                        ? "—"
                        : (keyLabel.get(x.uploader_key_id) ??
                          `${x.uploader_key_id.slice(0, KEY_ID_TRUNC)}…`)}
                    </TableCell>
                  </TableRow>
                )
              })}
          </TableBody>
        </Table>
      </div>

      {!isPending && !error && xorbs.length >= 500 && (
        <p className="mt-2 text-xs text-muted-foreground" data-testid="assets-pagination-note">
          Showing the first 500 rows. Narrow with search or the key filter.
        </p>
      )}
    </main>
  )
}

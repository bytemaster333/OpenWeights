import {
  ArrowLeftIcon,
  ArrowSquareOutIcon,
  CheckIcon,
  CircleNotchIcon,
  CloudCheckIcon,
  CloudSlashIcon,
  CopyIcon,
  HashIcon,
  InfoIcon,
} from "@phosphor-icons/react"
import { Link, useParams } from "@tanstack/react-router"
import { useEffect, useState } from "react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { useAsset } from "@/hooks/useAssets"
import { usePlatformSia } from "@/hooks/useSetupStatus"
import { formatBytes } from "@/lib/format"

/**
 * `/assets/$hash` — single-xorb detail page.
 *
 * Shape: hero (hash + copy + pin state badge) → info grid → Sia section
 * that explains pin state → referencing repos with deep-links into the
 * model catalog.*/

type PinMeta = {
  label: string
  variant: "default" | "secondary" | "destructive" | "outline"
  Icon: React.ComponentType<{ size?: number; weight?: "regular" | "bold" | "fill" | "light" | "thin" | "duotone" }>
  summary: string
  detail: string
}

function pinMeta(s: string): PinMeta {
  switch (s) {
    case "pinned":
      return {
        label: "Pinned on Sia",
        variant: "default",
        Icon: CloudCheckIcon,
        summary: "Durable on Sia.",
        detail: "Acknowledged by a Sia host.",
      }
    case "inline":
      return {
        label: "Stored inline",
        variant: "outline",
        Icon: InfoIcon,
        summary: "Stored inline in metadata DB.",
        detail:
          "Small LFS file kept as a Postgres BYTEA blob, not pinned to Sia. Bulk model bytes (xet xorbs) go through the Sia path.",
      }
    case "pinning":
      return {
        label: "Sia-pending",
        variant: "secondary",
        Icon: CircleNotchIcon,
        summary: "Waiting for a host contract.",
        detail: "Reconciler retries every minute.",
      }
    case "uploading":
      return {
        label: "Uploading",
        variant: "secondary",
        Icon: CircleNotchIcon,
        summary: "Bytes arriving.",
        detail: "Refresh in a few seconds.",
      }
    case "orphaned":
      return {
        label: "Orphaned",
        variant: "destructive",
        Icon: CloudSlashIcon,
        summary: "Retry budget exhausted.",
        detail: "Bytes still local. Retries resume when a host is available.",
      }
    default:
      return {
        label: s,
        variant: "outline",
        Icon: InfoIcon,
        summary: "Unknown pin state.",
        detail: "",
      }
  }
}

function CopyInline({ value, label }: { value: string; label: string }) {
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
      aria-label={copied ? `${label} copied` : `Copy ${label}`}
      onClick={() =>
        navigator.clipboard
          .writeText(value)
          .then(() => setCopied(true))
          .catch(() => {})
      }
    >
      {copied ? <CheckIcon size={12} /> : <CopyIcon size={12} />}
    </Button>
  )
}

export function AssetDetailPage() {
  const { hash } = useParams({ strict: false }) as { hash: string }
  const { data, isPending, error } = useAsset(hash)

  return (
    <main className="mx-auto max-w-5xl space-y-6 px-6 py-8">
      <Link
        to="/assets"
        className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
      >
        <ArrowLeftIcon size={14} />
        Back to assets
      </Link>

      {isPending && (
        <div className="space-y-4" data-testid="asset-detail-loading">
          <Skeleton className="h-10 w-1/2" />
          <Skeleton className="h-40 w-full" />
        </div>
      )}

      {!isPending && (error || !data) && (
        <div data-testid="asset-detail-not-found" className="rounded border border-destructive/40 bg-destructive/10 p-4 text-sm text-destructive">
          No xorb found for hash <code className="font-mono">{hash}</code>.
          {error ? ` (${error.message})` : null}
        </div>
      )}

      {!isPending && data && <AssetDetailBody data={data} />}
    </main>
  )
}

function AssetDetailBody({
  data,
}: {
  data: { xorb: import("@/hooks/useAssets").Xorb; referencing_repos: string[] }
}) {
  const x = data.xorb
  const pm = pinMeta(x.pin_state)
  const Icon = pm.Icon
  const { data: sia } = usePlatformSia()

  return (
    <>
      <header className="space-y-3">
        <div className="flex items-center gap-2 text-xs uppercase tracking-wide text-muted-foreground">
          <HashIcon size={14} weight="light" />
          Xorb
        </div>
        <div className="flex flex-wrap items-start gap-2">
          <h1
            className="break-all font-mono text-lg leading-snug"
            data-testid="asset-detail-hash"
          >
            {x.hash}
          </h1>
          <CopyInline value={x.hash} label="hash" />
        </div>
        <div className="flex items-center gap-2">
          <Icon size={16} weight="light" />
          <Badge variant={pm.variant}>{pm.label}</Badge>
          <span className="text-sm text-muted-foreground">{pm.summary}</span>
        </div>
      </header>

      <div className="grid gap-4 md:grid-cols-4">
        <InfoCard label="Size" value={formatBytes(x.size_bytes)} />
        <InfoCard label="Uploaded" value={new Date(x.uploaded_at).toLocaleString()} />
        <InfoCard
          label="Uploader key"
          value={
            x.uploader_key_id === "00000000-0000-0000-0000-000000000000" ? (
              <span className="text-xs text-muted-foreground">—</span>
            ) : (
              <code className="font-mono text-xs break-all">
                {x.uploader_key_id.slice(0, 13)}…
              </code>
            )
          }
        />
        <InfoCard
          label="Referenced by"
          value={`${data.referencing_repos.length} repo${
            data.referencing_repos.length === 1 ? "" : "s"
          }`}
        />
      </div>

      {/* Sia pin / inline storage section*/}
      <section className="rounded border bg-muted/10 p-4">
        <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {x.pin_state === "inline" ? "Storage" : "Sia pin"}
        </h2>
        {x.pin_state === "inline" ? (
          <div className="space-y-1">
            <div className="text-sm">Stored inline (Postgres BYTEA).</div>
            <p className="text-xs text-muted-foreground">{pm.detail}</p>
          </div>
        ) : x.sia_object_id ? (
          <div className="space-y-3">
            <div className="space-y-1">
              <div className="text-xs text-muted-foreground">Object ID</div>
              <div className="flex flex-wrap items-start gap-2">
                <code className="break-all font-mono text-xs">{x.sia_object_id}</code>
                <CopyInline value={x.sia_object_id} label="sia object id" />
              </div>
            </div>

            {/* Explorer drill-down — sia indexd doesn't expose a
                public per-object → contracts mapping, so we link the
                renter wallet (proves the renter exists on-chain) and the
                full deployment-wide contract pool (any of which COULD
                hold this xorb's slabs given erasure coding spreads
                shards across all formed contracts). Wording is honest
                about the deployment-wide scope so reviewers don't think
                the contract list is per-xorb. */}
            {sia && (
              <div className="space-y-2 border-t pt-3">
                <div className="text-xs text-muted-foreground">
                  Deployment Sia footprint — {sia.contract_count} contracts ·{" "}
                  {sia.distinct_host_count} hosts (any of these may hold this
                  xorb&apos;s shards)
                </div>
                <div className="flex flex-wrap gap-2">
                  {sia.wallet_address && (
                    <a
                      href={`${sia.siascan_base}/address/${sia.wallet_address}`}
                      target="_blank"
                      rel="noreferrer"
                      className="inline-flex items-center gap-1 rounded border px-2 py-1 text-xs hover:bg-muted/40"
                    >
                      Renter wallet
                      <ArrowSquareOutIcon size={12} weight="light" />
                    </a>
                  )}
                  {sia.contracts.slice(0, 6).map((c) => (
                    <a
                      key={c.id}
                      href={`${sia.siascan_base}/contract/${c.id}`}
                      target="_blank"
                      rel="noreferrer"
                      className="inline-flex items-center gap-1 rounded border px-2 py-1 font-mono text-xs hover:bg-muted/40"
                      title={`Contract ${c.id} · host ${c.host_key}`}
                    >
                      {c.id.slice(0, 8)}…
                      <ArrowSquareOutIcon size={12} weight="light" />
                    </a>
                  ))}
                </div>
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-1">
            <div className="text-sm">No Sia object id yet.</div>
            <p className="text-xs text-muted-foreground">{pm.detail}</p>
          </div>
        )}
      </section>

      {/* Referencing repos*/}
      {data.referencing_repos.length > 0 ? (
        <section
          data-testid="asset-detail-referencing-repos"
          className="rounded border bg-muted/10 p-4"
        >
          <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Referencing repos
          </h2>
          <ul className="space-y-1.5 text-sm">
            {data.referencing_repos.map((r) => {
              const [owner, name] = r.split("/")
              return (
                <li key={r}>
                  <Link
                    to="/models/$owner/$repo"
                    params={{ owner, repo: name }}
                    className="font-mono text-primary hover:underline"
                  >
                    {r}
                  </Link>
                </li>
              )
            })}
          </ul>
        </section>
      ) : (
        <section className="rounded border border-dashed p-4 text-center text-sm text-muted-foreground">
          Not referenced by any repo.
        </section>
      )}
    </>
  )
}

function InfoCard({
  label,
  value,
}: {
  label: string
  value: React.ReactNode
}) {
  return (
    <div className="rounded border bg-muted/10 p-3">
      <div className="text-xs uppercase tracking-wide text-muted-foreground">
        {label}
      </div>
      <div className="mt-1 text-sm">{value}</div>
    </div>
  )
}

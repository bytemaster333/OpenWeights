import { ArrowLeftIcon, CheckIcon, CopyIcon } from "@phosphor-icons/react"
import { Link, useParams } from "@tanstack/react-router"
import { useEffect, useState } from "react"

import { UserMenu } from "@/components/UserMenu"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { useAsset } from "@/hooks/useAssets"
import { useMe } from "@/hooks/useMe"

/**
 * `/assets/$hash` page (CONSOLE-06).
 *
 * Rendered when an operator clicks a hash in `/assets`. Shows the same
 * per-xorb fields exposed by `/admin/xorbs` plus a `referencing_repos`
 * list. Because 04-01 did not ship `/admin/xorbs/{hash}`, the `useAsset`
 * hook falls back to a filtered list scan (see `useAssets.ts` for the
 * story) — that fallback always returns `referencing_repos: []`, so the
 * "Referencing repos" section is hidden until a follow-up CAS plan lands
 * the detail endpoint with reconstruction-term joins.
 */

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

function CopyInlineButton({ value, label }: { value: string; label: string }) {
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
      data-testid={`asset-detail-copy-${label.replace(/\s+/g, "-").toLowerCase()}`}
      onClick={() => {
        navigator.clipboard
          .writeText(value)
          .then(() => setCopied(true))
          .catch(() => {
            // Clipboard denied — swallow.
          })
      }}
    >
      {copied ? <CheckIcon data-icon="inline-start" /> : <CopyIcon data-icon="inline-start" />}
      <span className="sr-only">Copy {label}</span>
    </Button>
  )
}

export function AssetDetailPage() {
  const { hash } = useParams({ strict: false }) as { hash: string }
  const { data: user } = useMe()
  const { data, isPending, error } = useAsset(hash)

  return (
    <main className="mx-auto max-w-3xl px-6 py-10">
      <header className="mb-6 flex items-center justify-between">
        <Link
          to="/assets"
          className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
          data-testid="asset-detail-back"
        >
          <ArrowLeftIcon className="size-4" aria-hidden="true" />
          Back to assets
        </Link>
        {user ? <UserMenu user={user} /> : null}
      </header>

      {isPending && (
        <div className="flex flex-col gap-4" data-testid="asset-detail-loading">
          <Skeleton className="h-8 w-3/4" />
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-5/6" />
          <Skeleton className="h-4 w-2/3" />
        </div>
      )}

      {!isPending && (error || !data) && (
        <div data-testid="asset-detail-not-found">
          <h1 className="mb-2 font-heading text-2xl font-medium">Not found</h1>
          <p className="text-sm text-muted-foreground">
            No xorb found for hash <code className="font-mono">{hash}</code>.
            {error ? ` (${error.message})` : null}
          </p>
        </div>
      )}

      {!isPending && data && (
        <article className="flex flex-col gap-6">
          <div>
            <p className="text-xs tracking-wide text-muted-foreground uppercase">Xorb</p>
            <div className="mt-1 flex items-start gap-2">
              <h1 className="font-mono text-base break-all" data-testid="asset-detail-hash">
                {data.xorb.hash}
              </h1>
              <CopyInlineButton value={data.xorb.hash} label="hash" />
            </div>
          </div>

          <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-3 text-sm">
            <dt className="text-muted-foreground">Sia object ID</dt>
            <dd className="font-mono text-xs break-all" data-testid="asset-detail-sia-object-id">
              {data.xorb.sia_object_id ? (
                <span className="inline-flex items-center gap-2">
                  <span>{data.xorb.sia_object_id}</span>
                  <CopyInlineButton value={data.xorb.sia_object_id} label="sia object id" />
                </span>
              ) : (
                <span className="text-muted-foreground">—</span>
              )}
            </dd>

            <dt className="text-muted-foreground">Size</dt>
            <dd data-testid="asset-detail-size">{formatBytes(data.xorb.size_bytes)}</dd>

            <dt className="text-muted-foreground">Pin state</dt>
            <dd>
              <Badge variant={pinStateVariant(data.xorb.pin_state)}>{data.xorb.pin_state}</Badge>
            </dd>

            <dt className="text-muted-foreground">Uploaded</dt>
            <dd data-testid="asset-detail-uploaded-at">
              {new Date(data.xorb.uploaded_at).toLocaleString()}
            </dd>

            <dt className="text-muted-foreground">Uploader key</dt>
            <dd className="font-mono text-xs" data-testid="asset-detail-uploader-key">
              {data.xorb.uploader_key_id}
            </dd>
          </dl>

          {data.referencing_repos.length > 0 && (
            <section data-testid="asset-detail-referencing-repos">
              <h2 className="mb-2 font-heading text-lg font-medium">Referencing repos</h2>
              <ul className="list-disc pl-6 text-sm">
                {data.referencing_repos.map((r) => (
                  <li key={r}>
                    <code className="font-mono">{r}</code>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </article>
      )}
    </main>
  )
}

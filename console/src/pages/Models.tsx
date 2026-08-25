import { ClockIcon, CloudCheckIcon, CubeIcon, DownloadIcon, FileIcon } from "@phosphor-icons/react"
import { Link } from "@tanstack/react-router"

import { Skeleton } from "@/components/ui/skeleton"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useModels } from "@/hooks/useModels"
import { usePlatformStats } from "@/hooks/useStats"
import { formatBytes, formatRelative } from "@/lib/format"

/**
 * `/models` — public catalog of OpenWeights-hosted models.
 *
 * Anonymous-readable surface (matches the backend `GET /api/models` which
 * hides `visibility != 'public'` rows). Clicking a row deep-links to
 * `/models/:owner/:repo` for the README + file table.*/

export function ModelsPage() {
  const { data, isLoading, isError, error } = useModels()
  const { data: platform } = usePlatformStats()

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 py-8">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <div className="flex items-center gap-3">
            <CubeIcon size={22} weight="light" className="text-muted-foreground" />
            <h1 className="font-heading text-2xl font-semibold tracking-tight">Models</h1>
          </div>
          <p className="mt-1 text-sm text-muted-foreground">
            Public models hosted on this OpenWeights deployment — byte-stored on Sia.
          </p>
        </div>
        {platform && platform.xorbs_total > 0 && (
          <Link
            to="/setup"
            className="inline-flex items-center gap-2 rounded border bg-muted/10 px-3 py-1.5 text-xs hover:border-primary/40 hover:bg-muted/30"
          >
            <CloudCheckIcon size={14} weight="light" className="text-primary" />
            <span>
              <span className="font-mono font-medium">{formatBytes(platform.bytes_on_sia)}</span> on
              Sia ·{" "}
              <span className="font-mono">
                {platform.xorbs_pinned}/{platform.xorbs_total}
              </span>{" "}
              xorbs
            </span>
          </Link>
        )}
      </header>

      {isLoading && (
        <div className="space-y-2">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
        </div>
      )}

      {isError && (
        <div className="rounded border border-destructive/40 bg-destructive/10 p-4 text-sm text-destructive">
          Failed to load models: {error?.message}
        </div>
      )}

      {data && data.length === 0 && (
        <div className="rounded border border-dashed p-12 text-center text-muted-foreground">
          <CubeIcon className="mx-auto mb-3" size={32} weight="light" />
          <p className="text-sm">No public models yet.</p>
          <p className="mt-2 text-xs">
            Publish one with:{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
              HF_ENDPOINT=https://hf.example.com hf upload &lt;owner&gt;/&lt;name&gt; ./files
            </code>
          </p>
        </div>
      )}

      {data && data.length > 0 && (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Model</TableHead>
              <TableHead className="text-right">Files</TableHead>
              <TableHead className="text-right">Size</TableHead>
              <TableHead className="text-right">Downloads</TableHead>
              <TableHead className="text-right">Updated</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.map((m) => {
              const [owner, name] = m.id.split("/")
              return (
                <TableRow key={m.id}>
                  <TableCell>
                    <Link
                      to="/models/$owner/$repo"
                      params={{ owner, repo: name }}
                      className="font-medium hover:underline"
                    >
                      {m.id}
                    </Link>
                    {m.description && (
                      <div className="text-xs text-muted-foreground">{m.description}</div>
                    )}
                  </TableCell>
                  <TableCell className="text-right text-sm text-muted-foreground">
                    <span className="inline-flex items-center gap-1">
                      <FileIcon size={14} weight="light" />
                      {m.files}
                    </span>
                  </TableCell>
                  <TableCell className="text-right text-sm text-muted-foreground">
                    {formatBytes(m.size)}
                  </TableCell>
                  <TableCell
                    className="text-right text-sm text-muted-foreground"
                    title={`${m.downloadsTotal} all-time`}
                  >
                    <span className="inline-flex items-center gap-1">
                      <DownloadIcon size={14} weight="light" />
                      {m.downloads30d}
                    </span>
                  </TableCell>
                  <TableCell className="text-right text-sm text-muted-foreground">
                    <span className="inline-flex items-center gap-1">
                      <ClockIcon size={14} weight="light" />
                      {formatRelative(m.lastModified)}
                    </span>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      )}
    </main>
  )
}

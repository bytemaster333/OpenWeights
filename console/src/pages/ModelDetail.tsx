import {
  ArrowSquareOutIcon,
  BroadcastIcon,
  CheckIcon,
  CopyIcon,
  DownloadIcon,
  FileIcon,
  HashIcon,
  TerminalIcon,
} from "@phosphor-icons/react"
import { Link, useParams } from "@tanstack/react-router"
import { useState } from "react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useMe } from "@/hooks/useMe"
import {
  type ModelInfo,
  useModelInfo,
  useModelObjects,
  useModelReadme,
  useModelTrend,
} from "@/hooks/useModels"
import { usePlatformSia } from "@/hooks/useSetupStatus"
import { formatBytes, formatRelative } from "@/lib/format"

/**
 * `/models/:owner/:repo` — single-model detail page.
 *
 * Layout mirrors huggingface.co's model page so reviewers see a familiar
 * surface while the bytes live on Sia:
 *
 * ┌─────────────────────────────────┬─────────────────┐
 * │ Hero: owner/repo + meta │ │
 * ├─────────────────────────────────┤ Sidebar │
 * │ "Use this model" tabbed card │ - file count │
 * │ (hf CLI · Python · curl) │ - total size │
 * ├─────────────────────────────────┤ - updated │
 * │ README (markdown rendered) │ - commit sha │
 * ├─────────────────────────────────┤ │
 * │ Files table w/ per-row download │ │
 * └─────────────────────────────────┴─────────────────┘
 *
 * The owner-only HF-announce card lives below the grid so it never
 * competes with the primary download UX.*/

const CAS_URL = import.meta.env.VITE_CAS_URL ?? "http://localhost:8080"

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export function ModelDetailPage() {
  const { owner, repo } = useParams({ strict: false }) as {
    owner: string
    repo: string
  }
  const { data: info, isLoading, isError, error } = useModelInfo(owner, repo)
  const { data: readme } = useModelReadme(owner, repo)
  const { data: me } = useMe()
  const isOwner = Boolean(me && me.login === owner)

  if (isLoading) {
    return (
      <div className="mx-auto max-w-6xl space-y-6 px-6 py-8">
        <Skeleton className="h-10 w-96" />
        <div className="grid gap-6 lg:grid-cols-[2fr_1fr]">
          <Skeleton className="h-64 w-full" />
          <Skeleton className="h-64 w-full" />
        </div>
      </div>
    )
  }

  if (isError || !info) {
    return (
      <div className="mx-auto max-w-6xl px-6 py-8">
        <div className="rounded border border-destructive/40 bg-destructive/10 p-4 text-sm text-destructive">
          {error?.status === 404
            ? `Model ${owner}/${repo} not found on this deployment.`
            : `Failed to load: ${error?.message ?? "unknown error"}`}
        </div>
        <Link to="/models" className="mt-4 inline-block text-sm text-primary hover:underline">
          ← Back to models
        </Link>
      </div>
    )
  }

  const totalBytes = info.siblings.reduce((n, s) => n + s.size, 0)

  return (
    <div className="mx-auto max-w-6xl space-y-6 px-6 py-8">
      {/* Breadcrumb*/}
      <Link
        to="/models"
        className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
      >
        ← All models
      </Link>

      {/* Hero*/}
      <header className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="font-heading text-3xl font-semibold tracking-tight">
            <Link to="/models" search={{}} className="text-muted-foreground hover:text-foreground">
              {info.author}
            </Link>
            <span className="text-muted-foreground"> / </span>
            <span>{repo}</span>
          </h1>
          <Badge variant={info.private ? "secondary" : "outline"}>
            {info.private ? "private" : "public"}
          </Badge>
          <Badge variant="outline">
            <HashIcon size={12} weight="light" className="mr-1" />
            xet · sia
          </Badge>
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span>
            Updated <span className="text-foreground">{formatRelative(info.lastModified)}</span>
          </span>
          <span>
            Commit <code className="text-foreground">{info.sha.slice(0, 12)}</code>
          </span>
          <span>
            <span className="text-foreground">{info.siblings.length}</span> files
          </span>
          <span>
            <span className="text-foreground">{formatBytes(totalBytes)}</span> total
          </span>
        </div>
      </header>

      {/* Main grid*/}
      <div className="grid gap-6 lg:grid-cols-[minmax(0,2fr)_1fr]">
        {/* Left column*/}
        <div className="space-y-6">
          <UseCard owner={owner} repo={repo} info={info} />

          {readme ? (
            <section className="rounded border bg-muted/10 p-6">
              <h2 className="mb-4 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                README.md
              </h2>
              <div className="readme-prose font-sans text-sm leading-7 text-foreground/90">
                <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
                  {stripFrontmatter(readme)}
                </ReactMarkdown>
              </div>
            </section>
          ) : (
            <section className="rounded border border-dashed p-6 text-center text-sm text-muted-foreground">
              No README.md in this revision.
            </section>
          )}

          <FilesSection owner={owner} repo={repo} info={info} />

          <ObjectsSection owner={owner} repo={repo} />
        </div>

        {/* Sidebar*/}
        <aside className="space-y-4">
          <SidebarCard title="At a glance">
            <SidebarRow label="Files" value={String(info.siblings.length)} />
            <SidebarRow label="Total size" value={formatBytes(totalBytes)} />
            <SidebarRow label="Downloads" value={String(info.downloads.total)} />
            <SidebarRow label="Updated" value={formatRelative(info.lastModified)} />
            <SidebarRow label="Visibility" value={info.private ? "Private" : "Public"} />
          </SidebarCard>

          <DownloadsCard owner={owner} repo={repo} info={info} />

          <SidebarCard title="Storage">
            <p className="text-xs text-muted-foreground">
              Weights on Sia, served via a Xet-compatible CAS. No huggingface.co round-trip.
            </p>
          </SidebarCard>
        </aside>
      </div>

      {/* Owner-only HF bridge*/}
      {isOwner && <HfBridgeCard owner={owner} repo={repo} />}
    </div>
  )
}

// ---------------------------------------------------------------------------
// "Use this model" tabbed card
// ---------------------------------------------------------------------------

type Tab = "cli" | "python" | "curl"

function UseCard({
  owner,
  repo,
  info,
}: {
  owner: string
  repo: string
  info: ModelInfo
}) {
  const [tab, setTab] = useState<Tab>("cli")
  const firstFile = info.siblings[0]?.rfilename ?? "model.safetensors"

  const cliCmd = `HF_ENDPOINT=${CAS_URL} hf download ${owner}/${repo}`
  const pythonCmd = `# pip install huggingface_hub
import os
os.environ["HF_ENDPOINT"] = "${CAS_URL}"

from huggingface_hub import snapshot_download
local_dir = snapshot_download("${owner}/${repo}")
print(f"Downloaded to {local_dir}")`

  const curlCmd = `# Fetch a single file directly via the HF-compat resolve endpoint
curl -L -o ${firstFile} \\
  "${CAS_URL}/${owner}/${repo}/resolve/main/${firstFile}"`

  const active = tab === "cli" ? cliCmd : tab === "python" ? pythonCmd : curlCmd

  return (
    <section className="rounded border bg-muted/20 p-4">
      <div className="mb-3 flex items-center justify-between">
        <h2 className="flex items-center gap-2 text-sm font-semibold">
          <TerminalIcon size={16} weight="light" /> Use this model
        </h2>
        <div className="flex gap-1">
          {(["cli", "python", "curl"] as const).map((t) => (
            <Button
              key={t}
              size="sm"
              variant={tab === t ? "default" : "ghost"}
              onClick={() => setTab(t)}
            >
              {t === "cli" ? "hf CLI" : t === "python" ? "Python" : "curl"}
            </Button>
          ))}
        </div>
      </div>
      <CodeBlock code={active} />
    </section>
  )
}

// ---------------------------------------------------------------------------
// Objects on Sia section — surfaces the underlying xorbs (and inline LFS
// blobs) this model's HEAD commit references. Each row is a content-
// addressed blob with a pin-state pill, the per-host shard count, and a
// click-through to /assets/{hash} for the full siascan drilldown.
// ---------------------------------------------------------------------------

function ObjectsSection({ owner, repo }: { owner: string; repo: string }) {
  const { data: objects, isPending } = useModelObjects(owner, repo)
  const { data: sia } = usePlatformSia()

  const totals = (objects ?? []).reduce(
    (acc, o) => {
      acc.size += o.size_bytes
      acc.count += 1
      if (o.pin_state === "pinned") {
        acc.pinned += 1
        acc.pinnedSize += o.size_bytes
      }
      return acc
    },
    { count: 0, size: 0, pinned: 0, pinnedSize: 0 },
  )

  return (
    <section className="rounded border bg-muted/10 p-6">
      <div className="mb-4 flex items-baseline justify-between gap-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Objects on Sia
        </h2>
        {objects && objects.length > 0 && (
          <span className="text-xs text-muted-foreground">
            <span className="font-mono text-foreground">
              {totals.pinned}/{totals.count}
            </span>{" "}
            pinned ·{" "}
            <span className="font-mono text-foreground">{formatBytes(totals.pinnedSize)}</span> on
            Sia
          </span>
        )}
      </div>

      {isPending && <Skeleton className="h-32 w-full" />}

      {!isPending && (!objects || objects.length === 0) && (
        <div className="py-6 text-center text-xs text-muted-foreground">
          No objects in this revision.
        </div>
      )}

      {!isPending && objects && objects.length > 0 && (
        <div className="overflow-x-auto rounded border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Object</TableHead>
                <TableHead>Pin state</TableHead>
                <TableHead>Files</TableHead>
                <TableHead className="text-right">Size</TableHead>
                <TableHead className="text-right">Sia</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {objects.map((o) => (
                <TableRow key={o.hash}>
                  <TableCell>
                    <Link
                      to="/assets/$hash"
                      params={{ hash: o.hash }}
                      className="font-mono text-xs text-primary hover:underline"
                      title={o.hash}
                    >
                      {o.hash.slice(0, 16)}…
                    </Link>
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={
                        o.pin_state === "pinned"
                          ? "default"
                          : o.pin_state === "inline"
                            ? "outline"
                            : o.pin_state === "orphaned"
                              ? "destructive"
                              : "secondary"
                      }
                    >
                      {o.pin_state}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-xs">
                    {o.files.length === 1 ? (
                      <code className="font-mono text-muted-foreground">{o.files[0]}</code>
                    ) : (
                      <span className="text-muted-foreground" title={o.files.join("\n")}>
                        {o.files.length} files
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatBytes(o.size_bytes)}
                  </TableCell>
                  <TableCell className="text-right">
                    {o.pin_state === "pinned" && sia ? (
                      <a
                        href={`${sia.siascan_base}/address/${sia.wallet_address}`}
                        target="_blank"
                        rel="noreferrer"
                        className="inline-flex items-center gap-1 text-xs text-primary underline-offset-2 hover:underline"
                        title="View renter wallet on siascan"
                      >
                        <ArrowSquareOutIcon size={12} weight="light" />
                      </a>
                    ) : (
                      <span className="text-muted-foreground/50">—</span>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  )
}

// ---------------------------------------------------------------------------
// Files section
// ---------------------------------------------------------------------------

function FilesSection({
  owner,
  repo,
  info,
}: {
  owner: string
  repo: string
  info: ModelInfo
}) {
  return (
    <section className="rounded border">
      <div className="border-b px-4 py-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Files ({info.siblings.length})
      </div>
      {info.siblings.length === 0 ? (
        <div className="p-6 text-center text-sm text-muted-foreground">
          This revision has no files.
        </div>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Path</TableHead>
              <TableHead className="text-right">Size</TableHead>
              <TableHead>Hash</TableHead>
              <TableHead className="w-24 text-right">Download</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {info.siblings.map((s) => (
              <TableRow key={s.rfilename}>
                <TableCell className="font-mono text-sm">
                  <span className="inline-flex items-center gap-2">
                    <FileIcon size={14} weight="light" />
                    {s.rfilename}
                  </span>
                </TableCell>
                <TableCell className="text-right text-sm text-muted-foreground">
                  {formatBytes(s.size)}
                </TableCell>
                <TableCell className="font-mono text-xs text-muted-foreground">
                  {s.blobId ? <CopyableHash value={s.blobId} /> : "—"}
                </TableCell>
                <TableCell className="text-right">
                  <a
                    href={`${CAS_URL}/${owner}/${repo}/resolve/main/${encodeURIComponent(
                      s.rfilename,
                    )}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    <Button variant="ghost" size="sm">
                      <DownloadIcon size={14} />
                    </Button>
                  </a>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </section>
  )
}

function CopyableHash({ value }: { value: string }) {
  const [copied, setCopied] = useState(false)
  const short = value.length > 12 ? `${value.slice(0, 12)}…` : value
  return (
    <button
      type="button"
      onClick={async () => {
        await navigator.clipboard.writeText(value)
        setCopied(true)
        setTimeout(() => setCopied(false), 1200)
      }}
      className="inline-flex items-center gap-1 rounded px-1 py-0.5 hover:bg-muted"
      title="Copy full hash"
    >
      {short}
      {copied ? <CheckIcon size={10} /> : <CopyIcon size={10} className="opacity-60" />}
    </button>
  )
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

function SidebarCard({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <div className="rounded border bg-muted/10 p-4">
      <h3 className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      <div className="space-y-2 text-sm">{children}</div>
    </div>
  )
}

function SidebarRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium">{value}</span>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Downloads card (sidebar) — rolling counters + 14d trend sparkline
// ---------------------------------------------------------------------------

function DownloadsCard({
  owner,
  repo,
  info,
}: {
  owner: string
  repo: string
  info: ModelInfo
}) {
  const { data: trend, isLoading } = useModelTrend(owner, repo)

  const peak = (trend ?? []).reduce((m, d) => Math.max(m, d.count), 0)
  const hasAny = peak > 0

  return (
    <div className="rounded border bg-muted/10 p-4">
      <h3 className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Downloads · last 30 days
      </h3>
      <div className="grid grid-cols-2 gap-y-2 text-sm">
        <div>
          <div className="text-xs text-muted-foreground">24 h</div>
          <div className="text-lg font-semibold">{info.downloads.last_24h}</div>
        </div>
        <div>
          <div className="text-xs text-muted-foreground">7 d</div>
          <div className="text-lg font-semibold">{info.downloads.last_7d}</div>
        </div>
        <div>
          <div className="text-xs text-muted-foreground">30 d</div>
          <div className="text-lg font-semibold">{info.downloads.last_30d}</div>
        </div>
        <div>
          <div className="text-xs text-muted-foreground">All time</div>
          <div className="text-lg font-semibold">{info.downloads.total}</div>
        </div>
      </div>

      {/* 14-day sparkline. Pure SVG — avoids dragging recharts' 60kB
 for a decorative line. Empty state shows a flat dashed baseline.*/}
      <div className="mt-4">
        <div className="mb-1 flex items-baseline justify-between text-xs text-muted-foreground">
          <span>Last 14 days</span>
          {hasAny && <span>peak {peak}</span>}
        </div>
        {isLoading ? <Skeleton className="h-[48px] w-full" /> : <Sparkline data={trend ?? []} />}
      </div>
    </div>
  )
}

function Sparkline({ data }: { data: { day: string; count: number }[] }) {
  const W = 200
  const H = 48
  const P = 2
  const peak = Math.max(1, ...data.map((d) => d.count))

  if (data.length === 0) {
    return (
      <div className="flex h-[48px] w-full items-center justify-center text-xs text-muted-foreground">
        No data
      </div>
    )
  }

  const stepX = (W - 2 * P) / Math.max(1, data.length - 1)
  const points = data
    .map((d, i) => {
      const x = P + i * stepX
      const y = H - P - (d.count / peak) * (H - 2 * P)
      return `${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(" ")
  const areaPath = `M${P},${H - P} L${points.split(" ").join(" L")} L${W - P},${H - P} Z`

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      className="w-full"
      style={{ height: H }}
    >
      <title>Downloads over the last 14 days</title>
      <path d={areaPath} fill="currentColor" fillOpacity={0.12} />
      <polyline
        points={points}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

// ---------------------------------------------------------------------------
// Generic code block with copy button
// ---------------------------------------------------------------------------

function CodeBlock({ code }: { code: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded border bg-background p-3 font-mono text-xs">
        {code}
      </pre>
      <Button
        variant="ghost"
        size="sm"
        className="absolute right-1 top-1"
        onClick={async () => {
          await navigator.clipboard.writeText(code.replace(/\s+$/g, ""))
          setCopied(true)
          setTimeout(() => setCopied(false), 1500)
        }}
      >
        {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
        {copied ? " Copied" : " Copy"}
      </Button>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Markdown renderer overrides
// ---------------------------------------------------------------------------

// Strip a leading YAML frontmatter block (`---\n...\n---\n`) before rendering.
// HF model READMEs almost always start with a license/tags block that's not
// meant for the rendered card view. ReactMarkdown has no built-in frontmatter
// awareness — without this, the YAML lines render as a literal table or
// horizontal rule wall at the top of every card.
function stripFrontmatter(md: string): string {
  if (!md.startsWith("---")) return md
  const close = md.indexOf("\n---", 3)
  if (close < 0) return md
  return md.slice(close + 4).replace(/^\s*\n/, "")
}

const markdownComponents = {
  h1: ({ children }: { children?: React.ReactNode }) => (
    <h1 className="mb-4 mt-6 border-b pb-2 font-heading text-2xl font-semibold tracking-tight first:mt-0">
      {children}
    </h1>
  ),
  h2: ({ children }: { children?: React.ReactNode }) => (
    <h2 className="mb-3 mt-8 font-heading text-xl font-semibold tracking-tight first:mt-0">
      {children}
    </h2>
  ),
  h3: ({ children }: { children?: React.ReactNode }) => (
    <h3 className="mb-2 mt-6 font-heading text-base font-semibold tracking-tight first:mt-0">
      {children}
    </h3>
  ),
  h4: ({ children }: { children?: React.ReactNode }) => (
    <h4 className="mb-2 mt-4 font-heading text-sm font-semibold tracking-tight">{children}</h4>
  ),
  p: ({ children }: { children?: React.ReactNode }) => <p className="mb-4 leading-7">{children}</p>,
  a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
    <a
      href={href}
      className="text-primary underline underline-offset-2 hover:no-underline"
      target="_blank"
      rel="noreferrer"
    >
      {children}
    </a>
  ),
  ul: ({ children }: { children?: React.ReactNode }) => (
    <ul className="mb-4 list-disc space-y-1.5 pl-6 leading-7">{children}</ul>
  ),
  ol: ({ children }: { children?: React.ReactNode }) => (
    <ol className="mb-4 list-decimal space-y-1.5 pl-6 leading-7">{children}</ol>
  ),
  li: ({ children }: { children?: React.ReactNode }) => <li className="pl-1">{children}</li>,
  code: ({ inline, children }: { inline?: boolean; children?: React.ReactNode }) =>
    inline ? (
      <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em]">{children}</code>
    ) : (
      <code className="block font-mono text-xs leading-relaxed">{children}</code>
    ),
  pre: ({ children }: { children?: React.ReactNode }) => (
    <pre className="mb-4 overflow-x-auto rounded border bg-muted/40 p-4 font-mono text-xs leading-relaxed">
      {children}
    </pre>
  ),
  blockquote: ({ children }: { children?: React.ReactNode }) => (
    <blockquote className="mb-4 border-l-2 border-primary/40 bg-muted/20 px-4 py-2 italic text-muted-foreground">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-6 border-border" />,
  table: ({ children }: { children?: React.ReactNode }) => (
    <div className="mb-4 overflow-x-auto rounded border">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  ),
  thead: ({ children }: { children?: React.ReactNode }) => (
    <thead className="bg-muted/40">{children}</thead>
  ),
  th: ({ children }: { children?: React.ReactNode }) => (
    <th className="border-b border-border px-3 py-2 text-left font-semibold">{children}</th>
  ),
  td: ({ children }: { children?: React.ReactNode }) => (
    <td className="border-b border-border/50 px-3 py-2 align-top">{children}</td>
  ),
  img: ({ src, alt }: { src?: string; alt?: string }) => (
    <img src={src} alt={alt} className="mb-4 max-w-full rounded border" loading="lazy" />
  ),
  strong: ({ children }: { children?: React.ReactNode }) => (
    <strong className="font-semibold text-foreground">{children}</strong>
  ),
}

// ---------------------------------------------------------------------------
// HF.co announce bridge (owner-only)
// ---------------------------------------------------------------------------

type Shell = "bash" | "zsh" | "fish"

function HfBridgeCard({ owner, repo }: { owner: string; repo: string }) {
  const [copied, setCopied] = useState(false)
  const [shell, setShell] = useState<Shell>("bash")

  const readme = [
    "---",
    "license: apache-2.0",
    "tags:",
    "  - openweights",
    "  - xet",
    "  - sia",
    "  - decentralized-storage",
    "---",
    "",
    `# ${owner}/${repo}`,
    "",
    "**Hosted on OpenWeights — a Xet-compatible, Sia-backed distribution network.**",
    "",
    "This huggingface.co repository is a pointer for discovery. The model",
    "weights live on the decentralized Sia storage network and are served",
    "through a Xet-compatible CAS.",
    "",
    "## Download",
    "",
    "```bash",
    "pip install huggingface_hub",
    `HF_ENDPOINT=${CAS_URL} hf download ${owner}/${repo}`,
    "```",
    "",
    `Browse: ${CAS_URL}/models/${owner}/${repo}`,
  ].join("\n")

  const chain = shell === "fish" ? "; and \\" : "&& \\"
  const cmd = [
    `echo '${readme}' > README.md ${chain}`,
    `HF_ENDPOINT=https://huggingface.co hf repos create ${owner}/${repo} --type model --exist-ok ${chain}`,
    `HF_ENDPOINT=https://huggingface.co hf upload ${owner}/${repo} README.md`,
  ].join("\n")

  async function copyCmd() {
    await navigator.clipboard.writeText(cmd.replace(/\s+$/g, ""))
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <details className="rounded border bg-muted/20 p-4">
      <summary className="cursor-pointer select-none text-sm font-medium">
        <span className="inline-flex items-center gap-2">
          <BroadcastIcon size={16} weight="light" />
          Announce on huggingface.co
        </span>
      </summary>
      <div className="mt-3 space-y-2">
        <p className="text-xs text-muted-foreground">
          Pointer-only mirror on huggingface.co. Weights stay on Sia.
        </p>
        <div className="flex items-center gap-2">
          {(["bash", "zsh", "fish"] as const).map((s) => (
            <Button
              key={s}
              variant={shell === s ? "default" : "outline"}
              size="sm"
              onClick={() => setShell(s)}
            >
              {s}
            </Button>
          ))}
          <div className="ml-auto">
            <Button variant="ghost" size="sm" onClick={copyCmd}>
              {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
              {copied ? " Copied" : " Copy"}
            </Button>
          </div>
        </div>
        <pre className="overflow-x-auto rounded border bg-background p-3 font-mono text-xs">
          {cmd}
        </pre>
      </div>
    </details>
  )
}

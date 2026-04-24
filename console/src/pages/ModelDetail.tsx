import { Link, useParams } from "@tanstack/react-router"
import { useState } from "react"
import {
  BroadcastIcon,
  CheckIcon,
  CopyIcon,
  DownloadIcon,
  FileIcon,
  HashIcon,
  TerminalIcon,
} from "@phosphor-icons/react"
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
  useModelReadme,
  useModelTrend,
} from "@/hooks/useModels"
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
            <Link
              to="/models"
              search={{}}
              className="text-muted-foreground hover:text-foreground"
            >
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
              <div className="prose prose-sm prose-invert max-w-none">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={markdownComponents}
                >
                  {readme}
                </ReactMarkdown>
              </div>
            </section>
          ) : (
            <section className="rounded border border-dashed p-6 text-center text-sm text-muted-foreground">
              No README.md in this revision.
            </section>
          )}

          <FilesSection owner={owner} repo={repo} info={info} />
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
              Weights on Sia, served via a Xet-compatible CAS. No huggingface.co
              round-trip.
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
                  {s.blob_id ? (
                    <CopyableHash value={s.blob_id} />
                  ) : (
                    "—"
                  )}
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
        {isLoading ? (
          <Skeleton className="h-[48px] w-full" />
        ) : (
          <Sparkline data={trend ?? []} />
        )}
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

const markdownComponents = {
  h1: ({ children }: { children?: React.ReactNode }) => (
    <h1 className="mb-4 mt-2 text-2xl font-semibold">{children}</h1>
  ),
  h2: ({ children }: { children?: React.ReactNode }) => (
    <h2 className="mb-2 mt-5 text-lg font-semibold">{children}</h2>
  ),
  h3: ({ children }: { children?: React.ReactNode }) => (
    <h3 className="mb-2 mt-4 text-base font-semibold">{children}</h3>
  ),
  p: ({ children }: { children?: React.ReactNode }) => (
    <p className="mb-3 leading-relaxed text-foreground/90">{children}</p>
  ),
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
    <ul className="mb-3 list-disc space-y-1 pl-5">{children}</ul>
  ),
  ol: ({ children }: { children?: React.ReactNode }) => (
    <ol className="mb-3 list-decimal space-y-1 pl-5">{children}</ol>
  ),
  code: ({ inline, children }: { inline?: boolean; children?: React.ReactNode }) =>
    inline ? (
      <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
        {children}
      </code>
    ) : (
      <code className="block font-mono text-xs">{children}</code>
    ),
  pre: ({ children }: { children?: React.ReactNode }) => (
    <pre className="mb-3 overflow-x-auto rounded border bg-muted/30 p-3 font-mono text-xs">
      {children}
    </pre>
  ),
  blockquote: ({ children }: { children?: React.ReactNode }) => (
    <blockquote className="mb-3 border-l-2 border-border pl-3 italic text-muted-foreground">
      {children}
    </blockquote>
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
    "  - siahub",
    "  - xet",
    "  - sia",
    "  - decentralized-storage",
    "---",
    "",
    `# ${owner}/${repo}`,
    "",
    "**Hosted on SiaHub — a Xet-compatible, Sia-backed distribution network.**",
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

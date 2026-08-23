import { Link } from "@tanstack/react-router"

import { StatsTile } from "@/components/StatsTile"
import { UserMenu } from "@/components/UserMenu"
import { Button } from "@/components/ui/button"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useMe } from "@/hooks/useMe"
import { useBenchmarks } from "@/hooks/useStats"

/**
 * `/stats/benchmarks` (CONSOLE-08).
 *
 * Renders OpenWeights vs HF-native (S3 + CloudFront) throughput numbers across
 * three scenarios: `cold-cache`, `warm-cache`, `upload`. The data source is
 * the static `public/benchmarks.json` file (see `useBenchmarks` rationale).
 *
 * UX notes:
 * - We render three KPI tiles for "OpenWeights (MB/s)" at a glance AND a full
 * table with both openweights + baseline columns. The tiles are for the
 * "reviewer skim" scenario; the table is the exhaustive view.
 * - When a row has `openweights_mbps === null` the tile shows "—" (placeholder
 * state pre-Phase 5 — the JSON ships with all-null values so the UI
 * doesn't 404 before CI populates it).
 * - Footer links to `docs/benchmarks.md` (the authoritative report) so
 * anyone reading the page can see the methodology, fixture list, and
 * hardware notes.
 */

const SCENARIOS = ["cold-cache", "warm-cache", "upload"] as const
type Scenario = (typeof SCENARIOS)[number]

const SCENARIO_LABEL: Record<Scenario, string> = {
  "cold-cache": "Cold cache",
  "warm-cache": "Warm cache",
  upload: "Upload",
}

const SCENARIO_SUBTEXT: Record<Scenario, string> = {
  "cold-cache": "first fetch — disk LRU miss, read from Sia",
  "warm-cache": "second fetch — disk LRU hit on gateway",
  upload: "xorb write — client → CAS → Sia",
}

export function BenchmarksPage() {
  const { data: user } = useMe()
  const { data, isPending } = useBenchmarks()

  const rowFor = (scenario: Scenario) => data?.rows.find((r) => r.scenario === scenario)

  return (
    <main className="mx-auto max-w-5xl space-y-8 px-6 py-10">
      <header className="flex items-start justify-between gap-4">
        <div>
          <h1 className="font-heading text-2xl font-medium tracking-tight">Benchmarks</h1>
          <p className="mt-1 max-w-2xl text-xs text-muted-foreground">
            OpenWeights throughput vs HF-native (S3 + CloudFront) baseline across three
            representative scenarios. Numbers are produced by Phase 5 CI and written to{" "}
            <code className="font-mono">public/benchmarks.json</code>. See{" "}
            <a
              href="https://github.com/bytemaster333/openweights/blob/main/docs/benchmarks.md"
              className="underline underline-offset-4 hover:text-foreground"
            >
              docs/benchmarks.md
            </a>{" "}
            for methodology.
          </p>
          {data?.generated_at && (
            <p className="mt-2 font-mono text-xs text-muted-foreground">
              Last regenerated: {new Date(data.generated_at).toLocaleString()}
            </p>
          )}
        </div>
        {user && <UserMenu user={user} />}
      </header>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        {SCENARIOS.map((s) => {
          const row = rowFor(s)
          return (
            <StatsTile
              key={s}
              label={SCENARIO_LABEL[s]}
              loading={isPending}
              value={
                row?.openweights_mbps != null ? `${row.openweights_mbps.toFixed(1)} MB/s` : "—"
              }
              subtext={SCENARIO_SUBTEXT[s]}
            />
          )
        })}
      </div>

      <section>
        <header className="mb-3 flex items-center justify-between">
          <h2 className="font-heading text-base font-medium">Head-to-head</h2>
        </header>
        <div className="rounded-none bg-card ring-1 ring-foreground/10">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Scenario</TableHead>
                <TableHead className="text-right">OpenWeights (MB/s)</TableHead>
                <TableHead className="text-right">HF baseline (MB/s)</TableHead>
                <TableHead className="text-right">Ratio</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {isPending && (
                <TableRow>
                  <TableCell colSpan={4} className="text-center text-xs text-muted-foreground">
                    Loading…
                  </TableCell>
                </TableRow>
              )}
              {data?.rows.map((r) => {
                const ratio =
                  r.openweights_mbps != null && r.hf_baseline_mbps != null && r.hf_baseline_mbps > 0
                    ? r.openweights_mbps / r.hf_baseline_mbps
                    : null
                return (
                  <TableRow key={r.scenario}>
                    <TableCell className="font-medium capitalize">
                      {r.scenario.replace("-", " ")}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {r.openweights_mbps != null ? r.openweights_mbps.toFixed(1) : "—"}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {r.hf_baseline_mbps != null ? r.hf_baseline_mbps.toFixed(1) : "—"}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {ratio != null ? `${(ratio * 100).toFixed(0)}%` : "—"}
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </div>
      </section>

      <footer className="flex items-center justify-between border-t border-foreground/10 pt-6 text-xs text-muted-foreground">
        <p>
          Placeholder values shown until Phase 5 CI populates{" "}
          <code className="font-mono">public/benchmarks.json</code>.
        </p>
        <Button asChild variant="outline" size="sm">
          <Link to="/stats">← Back to stats</Link>
        </Button>
      </footer>
    </main>
  )
}

import { CheckCircle, Question, XCircle } from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { type ConformanceStatus, effectiveStatus, useConformance } from "@/hooks/useConformance"

/**
 * Header pill that broadcasts the result of the most recent conformance-harness
 * CI run (CONSOLE-13, grant v1 gate).
 *
 * - PASS — green `default` badge.
 * - FAIL — red `destructive` badge.
 * - unknown / stale — neutral `secondary` grey badge.
 *
 * "Stale" is bucketed into "unknown" by `effectiveStatus()` — if the last CI
 * run timestamp is older than 24h (or absent), we deliberately do NOT keep
 * showing a stale PASS. This hedges against a CI outage silently vouching
 * for a release that may have regressed.
 *
 * A `title` tooltip exposes `last_run` + `commit` so the grant reviewer can
 * hover and see "last run: 2026-04-21 12:34 UTC, commit abc1234".
 */

const STATUS_COPY: Record<
  ConformanceStatus,
  { label: string; variant: "default" | "destructive" | "secondary" }
> = {
  pass: { label: "Conformance: PASS", variant: "default" },
  fail: { label: "Conformance: FAIL", variant: "destructive" },
  unknown: { label: "Conformance: unknown", variant: "secondary" },
}

export function ConformanceBadge() {
  const { data } = useConformance()
  const status = effectiveStatus(data)
  const copy = STATUS_COPY[status]

  // Tooltip is intentionally plain-text `title` (native) — no Radix tooltip
  // provider needed in the header, and keyboard-focus users get it via browser
  // default. Escape only a handful of fields so React doesn't double-quote.
  const tooltipLines: string[] = []
  if (data?.last_run) {
    tooltipLines.push(`Last run: ${new Date(data.last_run).toLocaleString()}`)
  }
  if (data?.commit) tooltipLines.push(`Commit: ${data.commit}`)
  if (data?.note && status === "unknown") tooltipLines.push(data.note)
  const title = tooltipLines.length > 0 ? tooltipLines.join("\n") : undefined

  return (
    <Badge
      variant={copy.variant}
      title={title}
      // Fade between status transitions — micro-animation allowance from
      // D-49. Uses tw-animate-css utilities that already ship with the preset.
      className="animate-in fade-in-0 duration-300"
      data-testid="conformance-badge"
      data-status={status}
    >
      {status === "pass" ? (
        <CheckCircle weight="fill" data-icon="inline-start" />
      ) : status === "fail" ? (
        <XCircle weight="fill" data-icon="inline-start" />
      ) : (
        <Question weight="bold" data-icon="inline-start" />
      )}
      {copy.label}
    </Badge>
  )
}

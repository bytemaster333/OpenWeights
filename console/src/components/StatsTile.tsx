import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"

/**
 * KPI tile for `/dashboard` and `/stats` (CONSOLE-07).
 *
 * Renders a compact `Card` with a muted label, a large tabular-nums value,
 * and an optional subtext. When `loading` is true, swaps the value for a
 * `Skeleton` so the grid does not jump layout between the initial render
 * and the first `/admin/stats` response (D-47 — 10s refetch means the
 * first paint is always `isPending: true`).
 *
 * Intentionally unopinionated about how `value` is formatted — callers
 * pass pre-formatted strings (e.g. `bytes(n)`, `%`, `String(n)`). This
 * keeps the component free of bytes/percentage helpers and makes tests
 * trivial (assert on the rendered string, not on a math helper).
 */
export function StatsTile({
  label,
  value,
  subtext,
  loading,
}: {
  label: string
  value: string
  subtext?: string
  loading?: boolean
}) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-xs font-normal text-muted-foreground">{label}</CardTitle>
      </CardHeader>
      <CardContent>
        {loading ? (
          <Skeleton className="h-8 w-24" />
        ) : (
          <div className="font-heading text-2xl font-medium tabular-nums">{value}</div>
        )}
        {subtext && <p className="mt-1 text-xs text-muted-foreground">{subtext}</p>}
      </CardContent>
    </Card>
  )
}

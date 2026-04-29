/**
 * Display-only formatting helpers shared across the console pages.
 *
 * Five page files had near-identical copies of these functions; keeping them
 * here deduplicates the drift (some rounded to 0 decimals, others to 2,
 * some used "min ago" vs "m ago"). Display decisions live in one place.*/

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const

/**
 * Human-readable byte size. Binary units (1024-based) — matches the rest of
 * the stack (`formatBytes` in CAS logs, `shard/xorb` size display). Uses
 * two decimals for sub-10 values (`2.45 MB`), zero for larger (`148 MB`).*/
export function formatBytes(n: number): string {
  let v = n
  let i = 0
  while (v >= 1024 && i < BYTE_UNITS.length - 1) {
    v /= 1024
    i += 1
  }
  return `${v.toFixed(v < 10 && i > 0 ? 2 : 0)} ${BYTE_UNITS[i]}`
}

/**
 * Relative-from-now label ("5 min ago", "3 h ago", "just now"). Accepts any
 * ISO-8601 string the backends emit. Returns "just now" for sub-minute
 * deltas; anything past a day collapses to "Nd ago".*/
export function formatRelative(iso: string): string {
  const d = new Date(iso).getTime()
  const diffSec = Math.round((Date.now() - d) / 1000)
  if (diffSec < 60) return "just now"
  if (diffSec < 3600) return `${Math.round(diffSec / 60)} min ago`
  if (diffSec < 86400) return `${Math.round(diffSec / 3600)} h ago`
  return `${Math.round(diffSec / 86400)} d ago`
}

/**
 * Full localised timestamp — preferred when the table column is "Time" and
 * the user expects an exact time rather than "5 min ago". Used by Stats'
 * activity table row hovers.*/
export function formatTimestamp(iso: string): string {
  return new Date(iso).toLocaleString()
}

/**
 * Format a hastings (Sia's atomic unit) value to a human-readable SC
 * string. 1 SC = 1e24 hastings. The CAS endpoint returns these as decimal
 * strings to preserve precision past JS number range.
 *
 * Rounded to 2 decimal places; appends " SC". Empty input → "—".
 */
export function formatHastingsToSC(hastings: string | null | undefined): string {
  if (!hastings) return "—"
  let h: bigint
  try {
    h = BigInt(hastings)
  } catch {
    return "—"
  }
  // 1 SC = 10^24 hastings. Compute SC * 100 (for 2-decimal precision) by
  // dividing by 10^22, then format with one decimal point.
  const scTimes100 = h / 10000000000000000000000n
  const intPart = scTimes100 / 100n
  const fracPart = scTimes100 % 100n
  return `${intPart}.${fracPart.toString().padStart(2, "0")} SC`
}

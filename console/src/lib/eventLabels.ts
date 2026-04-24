/**
 * `usage_log.event` values emitted by CAS + Gateway, mapped to a
 * human-readable label for table rendering.
 *
 * CAS emits: `xorb_upload`, `shard_upload`, `reconstruction`
 * Gateway emits: `download` (with `cache_hit = bool`)
 *
 * Unknown values fall back to the raw string so a new event type added in
 * CAS doesn't render as a mystery blank.*/

const LABELS: Record<string, string> = {
  xorb_upload: "Xorb upload",
  shard_upload: "Shard upload",
  reconstruction: "Reconstruction",
  download: "Download",
}

export function formatEvent(raw: string): string {
  return LABELS[raw] ?? raw
}

/**
 * `true` when the event carries a meaningful cache-hit semantic. Used to
 * decide whether to render the cache-hit column or a dash — upload events
 * are always NULL cache_hit and clutter the UI if rendered as "—".*/
export function eventHasCacheSemantics(raw: string): boolean {
  return raw === "download"
}

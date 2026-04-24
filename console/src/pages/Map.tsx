import {
  CheckIcon,
  CopyIcon,
  GlobeIcon,
  MagnifyingGlassIcon,
  MapPinIcon,
} from "@phosphor-icons/react"
import L from "leaflet"
import iconRetinaUrl from "leaflet/dist/images/marker-icon-2x.png"
import iconUrl from "leaflet/dist/images/marker-icon.png"
import shadowUrl from "leaflet/dist/images/marker-shadow.png"
import { useEffect, useMemo, useRef, useState } from "react"
import { MapContainer, Marker, Popup, TileLayer, useMap } from "react-leaflet"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Skeleton } from "@/components/ui/skeleton"
import { type MapHost, useMapHosts } from "@/hooks/useMap"

/**
 * `/stats/map` — world map of usable Sia storage hosts.
 *
 * Layout:
 * Left — dark-themed tile map (Carto "dark matter") with markers.
 * Right — filterable host list. Clicking a row pans/zooms the map to
 * that host and opens its popup.
 *
 * Marker icon fix — the default Leaflet icon assets are CSS-relative URLs
 * that Vite doesn't resolve at bundle time. Re-bind with Vite-imported
 * asset URLs so the blue pin renders.*/

L.Marker.prototype.options.icon = L.icon({
  iconUrl,
  iconRetinaUrl,
  shadowUrl,
  iconSize: [25, 41],
  iconAnchor: [12, 41],
  popupAnchor: [1, -34],
  shadowSize: [41, 41],
})

// Convert ISO-3166 alpha-2 to a flag emoji. Offsets: 'A' (0x41) → 🇦 (0x1F1E6).
function countryFlag(code: string | null): string {
  if (!code || code.length !== 2) return ""
  const cc = code.toUpperCase()
  return String.fromCodePoint(
    cc.charCodeAt(0) - 0x41 + 0x1f1e6,
    cc.charCodeAt(1) - 0x41 + 0x1f1e6,
  )
}

export function MapPage() {
  const { data: hosts, isPending, isError } = useMapHosts()
  const [query, setQuery] = useState("")
  const [selected, setSelected] = useState<string | null>(null)

  const filtered = useMemo(() => {
    if (!hosts) return []
    const q = query.trim().toLowerCase()
    if (!q) return hosts
    return hosts.filter(
      (h) =>
        h.public_key.toLowerCase().includes(q) ||
        (h.country_code?.toLowerCase() ?? "").includes(q),
    )
  }, [hosts, query])

  const countries = useMemo(() => {
    const m = new Map<string, number>()
    for (const h of hosts ?? []) {
      if (!h.country_code) continue
      m.set(h.country_code, (m.get(h.country_code) ?? 0) + 1)
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1])
  }, [hosts])

  const totalContracts = (hosts ?? []).reduce(
    (n, h) => n + (h.contract_count ?? 0),
    0,
  )

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 py-8">
      <header>
        <div className="flex items-center gap-3">
          <GlobeIcon size={22} weight="light" className="text-muted-foreground" />
          <h1 className="font-heading text-2xl font-semibold tracking-tight">
            Storage providers
          </h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">
          Sia hosts this deployment has contracts with.
        </p>
      </header>

      {/* Summary strip*/}
      <div className="grid gap-3 sm:grid-cols-3">
        <SummaryCard
          label="Usable hosts"
          value={hosts ? hosts.length.toString() : "—"}
          hint="contractable"
        />
        <SummaryCard
          label="Countries"
          value={countries.length.toString()}
          hint="distinct"
        />
        <SummaryCard
          label="Active contracts"
          value={totalContracts.toString()}
          hint="across all hosts"
        />
      </div>

      {/* Map + sidebar grid*/}
      <div className="grid gap-4 lg:grid-cols-[minmax(0,2fr)_1fr]">
        <div className="h-[560px] w-full overflow-hidden rounded-lg border bg-muted/10">
          {isPending && <Skeleton className="h-full w-full" data-testid="map-skeleton" />}
          {isError && !isPending && (
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              Unable to load hosts.
            </div>
          )}
          {!isPending && !isError && (
            <MapContainer
              center={[20, 0]}
              zoom={2}
              minZoom={2}
              scrollWheelZoom
              worldCopyJump
              style={{ height: "100%", width: "100%" }}
            >
              {/* Carto's "dark matter" tile set — attribution required.*/}
              <TileLayer
                url="https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png"
                attribution='&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> &copy; <a href="https://carto.com/attributions">CARTO</a>'
                subdomains="abcd"
                maxZoom={19}
              />
              <FlyToSelected hosts={hosts ?? []} selected={selected} />
              {hosts?.map((h) => (
                <Marker
                  key={h.public_key}
                  position={[h.lat, h.lon]}
                  eventHandlers={{
                    click: () => setSelected(h.public_key),
                  }}
                >
                  <Popup>
                    <HostPopup host={h} />
                  </Popup>
                </Marker>
              ))}
            </MapContainer>
          )}
        </div>

        <aside className="flex h-[560px] flex-col rounded-lg border bg-muted/10">
          <div className="border-b p-3">
            <div className="relative">
              <MagnifyingGlassIcon
                size={14}
                className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
              />
              <Input
                placeholder="Search host or country…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                className="h-8 pl-8 text-xs"
              />
            </div>
            <div className="mt-2 text-xs text-muted-foreground">
              {filtered.length} / {hosts?.length ?? 0} hosts
            </div>
          </div>
          <div className="flex-1 overflow-y-auto">
            {isPending && (
              <div className="space-y-2 p-3">
                <Skeleton className="h-12 w-full" />
                <Skeleton className="h-12 w-full" />
                <Skeleton className="h-12 w-full" />
              </div>
            )}
            {!isPending && filtered.length === 0 && (
              <div className="p-6 text-center text-xs text-muted-foreground">
                No hosts.
              </div>
            )}
            {!isPending &&
              filtered.map((h) => (
                <button
                  key={h.public_key}
                  onClick={() => setSelected(h.public_key)}
                  className={`block w-full border-b px-3 py-2 text-left text-xs transition-colors hover:bg-muted/40 ${
                    selected === h.public_key ? "bg-muted/60" : ""
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <MapPinIcon size={12} weight="light" className="text-muted-foreground" />
                    <span className="font-mono">
                      {h.public_key.slice(0, 16)}…
                    </span>
                    {h.country_code && (
                      <span className="ml-auto text-base leading-none">
                        {countryFlag(h.country_code)}
                      </span>
                    )}
                  </div>
                  <div className="mt-1 flex gap-3 text-[10px] text-muted-foreground">
                    <span>
                      {h.contract_count ?? 0} contract
                      {h.contract_count === 1 ? "" : "s"}
                    </span>
                    <span>
                      {h.lat.toFixed(2)}, {h.lon.toFixed(2)}
                    </span>
                  </div>
                </button>
              ))}
          </div>
        </aside>
      </div>
    </main>
  )
}

function SummaryCard({
  label,
  value,
  hint,
}: {
  label: string
  value: string
  hint: string
}) {
  return (
    <div className="rounded border bg-muted/10 p-4">
      <div className="text-xs uppercase tracking-wide text-muted-foreground">
        {label}
      </div>
      <div className="mt-1 text-2xl font-semibold">{value}</div>
      <div className="text-xs text-muted-foreground">{hint}</div>
    </div>
  )
}

function HostPopup({ host }: { host: MapHost }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="space-y-2 text-xs">
      <div className="flex items-start gap-2">
        <code className="break-all font-mono">{host.public_key}</code>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            navigator.clipboard.writeText(host.public_key).then(() => {
              setCopied(true)
              setTimeout(() => setCopied(false), 1500)
            })
          }}
        >
          {copied ? <CheckIcon size={10} /> : <CopyIcon size={10} />}
        </Button>
      </div>
      <div className="flex flex-wrap gap-2">
        <Badge variant={host.usable ? "default" : "destructive"}>
          {host.usable ? "usable" : "offline"}
        </Badge>
        {host.country_code && (
          <Badge variant="outline">
            <span className="mr-1">{countryFlag(host.country_code)}</span>
            {host.country_code}
          </Badge>
        )}
        {host.contract_count !== null && (
          <Badge variant="outline">
            {host.contract_count} contract{host.contract_count === 1 ? "" : "s"}
          </Badge>
        )}
      </div>
      <div className="text-[10px] text-muted-foreground">
        {host.lat.toFixed(4)}, {host.lon.toFixed(4)}
      </div>
    </div>
  )
}

/**
 * Imperative map controller — when `selected` changes, fly to that host's
 * coords. Uses the `useMap` hook (only valid inside `<MapContainer>`).*/
function FlyToSelected({
  hosts,
  selected,
}: {
  hosts: MapHost[]
  selected: string | null
}) {
  const map = useMap()
  const lastRef = useRef<string | null>(null)

  useEffect(() => {
    if (!selected || selected === lastRef.current) return
    const h = hosts.find((x) => x.public_key === selected)
    if (!h) return
    lastRef.current = selected
    map.flyTo([h.lat, h.lon], Math.max(map.getZoom(), 5), { duration: 0.8 })
  }, [selected, hosts, map])

  return null
}

import L from "leaflet"
import iconRetinaUrl from "leaflet/dist/images/marker-icon-2x.png"
import iconUrl from "leaflet/dist/images/marker-icon.png"
import shadowUrl from "leaflet/dist/images/marker-shadow.png"
import { MapContainer, Marker, Popup, TileLayer } from "react-leaflet"

import { Skeleton } from "@/components/ui/skeleton"
import { useMapHosts } from "@/hooks/useMap"

/**
 * CONSOLE-09 — `/stats/map`. Leaflet world map with a marker per usable Sia
 * storage host, sourced from CAS `/admin/stats/map` (indexd `/api/hosts`
 * proxied + GeoIP-enriched; D-42). Tiles are OpenStreetMap — no API key.
 *
 * Marker-icon asset fix (D-43 / common bundler gotcha):
 *   Leaflet's default marker icon ships as a CSS-relative URL that Vite does
 *   not resolve at bundle time, so the blue pin appears broken. Re-binding
 *   `L.Marker.prototype.options.icon` to a freshly-constructed `L.icon` whose
 *   URLs are resolved through Vite's asset pipeline (via `import ... from`)
 *   is the canonical workaround. Kept at module scope so it runs exactly
 *   once per app load.
 *
 * OSM tile usage policy (D-43):
 *   Browsers set their own `User-Agent`, un-overridable per-request. The
 *   attribution clause is satisfied by the `TileLayer.attribution` prop
 *   below. No further work required.
 */

L.Marker.prototype.options.icon = L.icon({
  iconUrl,
  iconRetinaUrl,
  shadowUrl,
  iconSize: [25, 41],
  iconAnchor: [12, 41],
  popupAnchor: [1, -34],
  shadowSize: [41, 41],
})

export function MapPage() {
  const { data: hosts, isPending, isError } = useMapHosts()

  return (
    <main className="mx-auto max-w-6xl space-y-4 px-6 py-10">
      <div className="flex items-baseline justify-between">
        <h1 className="font-semibold text-2xl">Storage providers</h1>
        {hosts && (
          <span className="text-muted-foreground text-sm">
            {hosts.length} usable {hosts.length === 1 ? "host" : "hosts"}
          </span>
        )}
      </div>

      <div className="h-[560px] w-full overflow-hidden rounded-lg border">
        {isPending && <Skeleton className="h-full w-full" data-testid="map-skeleton" />}
        {isError && !isPending && (
          <div className="flex h-full items-center justify-center text-muted-foreground text-sm">
            Unable to load hosts.
          </div>
        )}
        {!isPending && !isError && (
          <MapContainer
            center={[20, 0]}
            zoom={2}
            scrollWheelZoom
            style={{ height: "100%", width: "100%" }}
          >
            <TileLayer
              url="https://tile.openstreetmap.org/{z}/{x}/{y}.png"
              attribution='&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
            />
            {hosts?.map((h) => (
              <Marker key={h.public_key} position={[h.lat, h.lon]}>
                <Popup>
                  <div className="space-y-1 text-xs">
                    <div className="break-all font-mono">
                      {h.public_key.length > 24 ? `${h.public_key.slice(0, 24)}…` : h.public_key}
                    </div>
                    {h.country_code && <div>Country: {h.country_code}</div>}
                    {h.contract_count !== null && <div>Contracts: {h.contract_count}</div>}
                    <div>Status: {h.usable ? "usable" : "offline"}</div>
                  </div>
                </Popup>
              </Marker>
            ))}
          </MapContainer>
        )}
      </div>
    </main>
  )
}

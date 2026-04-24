import { useQuery } from "@tanstack/react-query"

import { casFetch } from "@/lib/api"

/**
 * Per-host shape returned by CAS `GET /admin/stats/map` (see Task 6).
 *
 * flow: browser → CAS `/admin/stats/map` → CAS proxies indexd
 * `/api/hosts?usable=true` (admin-auth'd, internal network only) →
 * CAS re-shapes into this canonical form. The browser never talks to indexd
 * directly; `INDEXD_ADMIN_PASSWORD` stays server-side.
 *
 * `lat` / `lon` come from indexd's GeoLite2-City.mmdb lookup at startup;
 * `country_code` is ISO 3166-1 alpha-2 (may be null for private-IP hosts).*/
export type MapHost = {
  public_key: string
  country_code: string | null
  lat: number
  lon: number
  usable: boolean
  contract_count: number | null
}

type MapResponse = { hosts: MapHost[] }

/**
 * TanStack Query hook for the geomap page.
 *
 * Refetch cadence is 60s ( allows 10s for `/stats`; hosts change on the
 * hour-scale, so 60s is polite toward both the CAS and the indexd cache).
 * Background refetch is disabled so hidden tabs don't burn CPU.*/
export function useMapHosts() {
  return useQuery<MapHost[]>({
    queryKey: ["map-hosts"],
    queryFn: () => casFetch<MapResponse>("/admin/stats/map").then((r) => r.hosts),
    refetchInterval: 60_000,
    refetchIntervalInBackground: false,
  })
}

import { Outlet, createRootRoute, createRoute, createRouter } from "@tanstack/react-router"

import { AuthGuard } from "@/components/AuthGuard"
import { Header } from "@/components/Header"
import { AssetDetailPage } from "@/pages/AssetDetail"
import { AssetsPage } from "@/pages/Assets"
import { DashboardPage } from "@/pages/Dashboard"
import { KeysPage } from "@/pages/Keys"
import { LandingPage } from "@/pages/Landing"
import { LoginPage } from "@/pages/Login"
import { MapPage } from "@/pages/Map"
import { ModelDetailPage } from "@/pages/ModelDetail"
import { ModelsPage } from "@/pages/Models"
import { SetupPage } from "@/pages/Setup"
import { StatsPage } from "@/pages/Stats"

/**
 * TanStack Router — code-based route definitions (D-40).
 *
 * Rationale: ~11 routes total; explicit `router.tsx` tree is a reviewer
 * legibility win and avoids the `@tanstack/router-vite-plugin` codegen step.
 *
 * Plans 04-03..04-09 add routes here; each plan's commit should be labeled
 * `router: add <path> route` to minimize merge conflicts across waves.
 *
 * Current routes:
 * - `/` Landing page + "Sign in with GitHub" CTA
 * - `/login` Alias that redirects to CAS `/auth/github/start`
 * - `/dashboard` Auth-gated welcome + KPI tiles (04-07)
 * - `/keys` Auth-gated: API key CRUD + usage snippets
 * (merged `/onboarding` flow; CONSOLE-02 + KEYS-01..04)
 * - `/assets` Auth-gated xorb catalog (04-06)
 * - `/assets/$hash` Auth-gated single-xorb detail (04-06)
 * - `/stats` Auth-gated live KPI (04-07)
 * - `/stats/map` Auth-gated Leaflet map (04-08)
 * - `/setup` Admin-gated operator diagnostics (04-09)
 *
 * The root route wraps the whole tree in `<Header>` + `<Outlet>`; the header
 * self-hides on landing/login via `useMe`, so the chrome is present exactly
 * where it should be.
 */

const rootRoute = createRootRoute({
  // `<Header>` self-hides for unauth'd users (useMe === null), so mounting
  // it here means every auth'd page renders the chrome without per-page wiring
  // and landing/login stay chrome-less — see Header.tsx docstring.
  component: () => (
    <>
      <Header />
      <Outlet />
    </>
  ),
})

const landingRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: LandingPage,
  // Landing reads `?error=<code>` off the URL (P14 surfacing) via
  // `useSearch({ strict: false })`, so no validated `validateSearch` here —
  // unrecognized codes are filtered client-side, not type-pruned.
})

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: LoginPage,
})

const dashboardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/dashboard",
  component: () => (
    <AuthGuard>
      <DashboardPage />
    </AuthGuard>
  ),
})

// /keys — auth-gated. Merged page combining key CRUD (KEYS-01..04) AND
// the former `/onboarding` flow (CONSOLE-02, KEYS-01/02). Single surface
// for "here's how to use OpenWeights with hf CLI".
const keysRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/keys",
  component: () => (
    <AuthGuard>
      <KeysPage />
    </AuthGuard>
  ),
})

// /assets — auth-gated. Admin xorb catalog with hash-prefix search and
// API-key filter (04-06, CONSOLE-03/-04/-05).
const assetsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/assets",
  component: () => (
    <AuthGuard>
      <AssetsPage />
    </AuthGuard>
  ),
})

// /assets/$hash — auth-gated detail drill-in for a single xorb (04-06,
// CONSOLE-06). `$hash` is a 64-char hex string; typed param available via
// `useParams({ strict: false })` inside `<AssetDetailPage>`.
const assetDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/assets/$hash",
  component: () => (
    <AuthGuard>
      <AssetDetailPage />
    </AuthGuard>
  ),
})

// /stats — auth-gated. Live KPI tiles + per-key usage + recent activity
// (04-07, CONSOLE-07). TanStack Query refetches every 10s (D-47).
const statsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/stats",
  component: () => (
    <AuthGuard>
      <StatsPage />
    </AuthGuard>
  ),
})

// /stats/map — auth-gated. Leaflet world map of Sia storage hosts
// (04-08, CONSOLE-09). Nested under `/stats`; 04-07 owns `/stats` root
// and `/stats/benchmarks`. This plan only registers the map sub-route.
const mapRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/stats/map",
  component: () => (
    <AuthGuard>
      <MapPage />
    </AuthGuard>
  ),
})

// /setup — diagnostic page (CONSOLE-11/12, OPS-01). Originally admin-gated
// but demoted to session-only: the page shows service health tiles + indexer
// URL + V2 flag — nothing sensitive — and reviewers expect to see it
// after signing in. CAS matches this on the server side (no is_admin check).
const setupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/setup",
  component: () => (
    <AuthGuard>
      <SetupPage />
    </AuthGuard>
  ),
})

// /models — public model catalog (HF-API-compat hub surface, migration
// 0008). Anonymous-readable: no `<AuthGuard>` wrapper. The list + detail
// pages render the same data hf_hub would pull via the REST API.
const modelsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/models",
  component: ModelsPage,
})

// /models/$owner/$repo — single-model detail (README + file list + copy
// command). Also public; downloads go through `/resolve/main/...`.
const modelDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/models/$owner/$repo",
  component: ModelDetailPage,
})

const routeTree = rootRoute.addChildren([
  landingRoute,
  loginRoute,
  dashboardRoute,
  keysRoute,
  assetsRoute,
  assetDetailRoute,
  statsRoute,
  mapRoute,
  setupRoute,
  modelsRoute,
  modelDetailRoute,
])

export const router = createRouter({ routeTree })

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router
  }
}

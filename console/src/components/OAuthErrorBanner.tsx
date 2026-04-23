import { WarningOctagon } from "@phosphor-icons/react"

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"

/**
 * P14 mitigation — self-host OAuth friction surfacing.
 *
 * The failure modes below are the ones CAS's `CallbackError::into_response`
 * (`cas/crates/siahub-cas-core/src/handlers/auth/github.rs`) emits as stable
 * `code` strings, plus two additional operator-config codes reserved for
 * `/setup` (Phase 5 work) to surface when the `.env` GitHub OAuth app
 * registration is missing or misconfigured. See 04-CONTEXT §3 and §7.
 *
 * Every code MUST have a map entry. If CAS later adds a code, plan-checker
 * flags a gap against `handlers/auth/github.rs`.
 */
export type OAuthErrorCode =
  | "oauth_state_mismatch"
  | "oauth_code_missing"
  | "github_token_exchange_failed"
  | "oauth_client_not_configured"
  | "oauth_callback_host_mismatch"

type ErrorCopy = {
  title: string
  description: string
  hint: string
}

/**
 * Maps each P14 error code to display copy. `hint` always references the
 * exact `.env` var or OAuth app field the operator should check, per
 * DOCS-02 promise (Phase 6).
 */
const ERROR_COPY: Record<OAuthErrorCode, ErrorCopy> = {
  oauth_state_mismatch: {
    title: "OAuth state mismatch",
    description:
      "The state token from GitHub did not match a pending sign-in. Either the sign-in was older than 10 minutes, or this is a forged callback.",
    hint: "Try signing in again from this page. If it keeps failing, clear cookies for the CAS origin and retry.",
  },
  oauth_code_missing: {
    title: "OAuth code missing",
    description:
      "GitHub redirected back without an authorization code. This usually means you denied the consent screen.",
    hint: "Click Sign in with GitHub again and approve the consent screen.",
  },
  github_token_exchange_failed: {
    title: "GitHub token exchange failed",
    description:
      "CAS could not exchange the authorization code for an access token. The GitHub OAuth app configuration is likely wrong.",
    hint: "Operator: verify GITHUB_OAUTH_CLIENT_ID and GITHUB_OAUTH_CLIENT_SECRET in .env match the GitHub OAuth app, and that the app is not suspended.",
  },
  oauth_client_not_configured: {
    title: "GitHub OAuth is not configured",
    description: "This SiaHub deployment has no GitHub OAuth credentials in its environment.",
    hint: "Operator: set GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET, and GITHUB_OAUTH_CALLBACK_URL in .env, then restart the CAS service.",
  },
  oauth_callback_host_mismatch: {
    title: "OAuth callback host mismatch",
    description:
      "The callback URL that GitHub hit does not match GITHUB_OAUTH_CALLBACK_URL in the deployment environment.",
    hint: "Operator: the GitHub OAuth app's Authorization callback URL must be exactly ${CAS_URL}/auth/github/callback and match GITHUB_OAUTH_CALLBACK_URL in .env.",
  },
}

export function OAuthErrorBanner({ code }: { code: OAuthErrorCode }) {
  const copy = ERROR_COPY[code]
  return (
    <Alert
      variant="destructive"
      // Subtle fade-in only — D-49 allows Framer Motion when a specific
      // component calls for micro-animation, but here a CSS keyframe is
      // cheaper and ships zero JS. The animation is defined in index.css
      // against the `animate-in fade-in` convention tw-animate-css provides.
      className="animate-in fade-in-0 slide-in-from-top-1 duration-300"
      data-testid="oauth-error-banner"
      data-error-code={code}
    >
      <WarningOctagon weight="fill" />
      <AlertTitle>{copy.title}</AlertTitle>
      <AlertDescription>
        <p>{copy.description}</p>
        <p className="font-medium">{copy.hint}</p>
        <p className="font-mono text-[0.7rem]">
          Error code: <span className="font-bold">{code}</span>
        </p>
      </AlertDescription>
    </Alert>
  )
}

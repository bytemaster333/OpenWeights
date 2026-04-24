import { Component, type ErrorInfo, type PropsWithChildren } from "react"

type State = { hasError: boolean; message?: string; requestId?: string }

/**
 * Top-level React error boundary.
 *
 * Logs to `console.error` only — NO Sentry, Rollbar, LogRocket, Datadog, or
 * any error-aggregation SaaS. Per-request API errors are surfaced via toasts
 * (wired in feature plans via Sonner); this boundary handles render crashes.*/
export class AppErrorBoundary extends Component<PropsWithChildren, State> {
  state: State = { hasError: false }

  static getDerivedStateFromError(err: Error): State {
    return { hasError: true, message: err.message }
  }

  componentDidCatch(err: Error, info: ErrorInfo) {
    // : console.error only. No Sentry, no Rollbar.
    console.error("[AppErrorBoundary]", err, info)
  }

  render() {
    if (this.state.hasError) {
      return (
        <div
          style={{
            padding: 32,
            fontFamily: "system-ui, -apple-system, sans-serif",
            maxWidth: 640,
            margin: "4rem auto",
          }}
        >
          <h1 style={{ fontSize: "1.5rem", marginBottom: "0.5rem" }}>Something went wrong.</h1>
          <p style={{ color: "#666", marginBottom: "1rem" }}>{this.state.message}</p>
          {this.state.requestId && (
            <code style={{ fontSize: "0.85rem" }}>Request ID: {this.state.requestId}</code>
          )}
        </div>
      )
    }
    return this.props.children
  }
}

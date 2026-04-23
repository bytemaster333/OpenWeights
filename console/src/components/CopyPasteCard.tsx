import { CheckIcon, CopyIcon } from "@phosphor-icons/react"
import { useEffect, useState } from "react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

/**
 * Single copy-paste block for the onboarding page.
 *
 * - `title`: human label shown above the block ("1. Set Xet env vars").
 * - `body`: the literal text that lands on the clipboard AND in the
 *   rendered `<pre>`. For onboarding, this includes the plaintext API key
 *   inlined into the `HF_XET_DATA_CUSTOM_HEADERS` env var.
 * - `hint`: optional footer line, e.g. "Replace <your-repo> with your own."
 *
 * Lifecycle note (D-45): the plaintext key reaches this component **only**
 * as part of `body: string` (by reference via `useState` in `OnboardingPage`).
 * Nothing here writes to `localStorage` / `sessionStorage` / analytics.
 */
export type CopyPasteCardProps = {
  title: string
  body: string
  hint?: string
  /** Optional data-testid for integration tests. */
  testId?: string
}

export function CopyPasteCard({ title, body, hint, testId }: CopyPasteCardProps) {
  const [copied, setCopied] = useState(false)

  // Reset the "copied" indicator 2s after the last copy so that re-copying
  // feels responsive if the user clicks again.
  useEffect(() => {
    if (!copied) return
    const t = window.setTimeout(() => setCopied(false), 2000)
    return () => window.clearTimeout(t)
  }, [copied])

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(body)
      setCopied(true)
      toast.success("Copied to clipboard")
    } catch {
      toast.error("Clipboard unavailable — select the text manually")
    }
  }

  return (
    <div data-slot="copy-paste-card" data-testid={testId} className="rounded-none border bg-card">
      <div className="flex items-center justify-between border-b px-3 py-2">
        <h3 className="font-heading text-xs font-medium">{title}</h3>
        <Button
          variant="ghost"
          size="sm"
          onClick={copy}
          data-testid={testId ? `${testId}-copy` : undefined}
          aria-label={`Copy ${title}`}
        >
          {copied ? <CheckIcon data-icon="inline-start" /> : <CopyIcon data-icon="inline-start" />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre className={cn("overflow-x-auto px-3 py-3 font-mono text-xs/relaxed text-foreground")}>
        <code>{body}</code>
      </pre>
      {hint ? <p className="border-t px-3 py-2 text-xs text-muted-foreground">{hint}</p> : null}
    </div>
  )
}

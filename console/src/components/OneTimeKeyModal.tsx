import { WarningCircleIcon } from "@phosphor-icons/react"
import { useEffect, useState } from "react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

/**
 * One-time plaintext-key modal (D-45).
 *
 * Load-bearing invariants:
 *
 * 1. Plaintext lives in a **prop** — the parent owns state. We never copy
 * it to a ref, never to storage, never to analytics. The only DOM
 * imprint is the `<code>` element while `open={true}`.
 * 2. While `open && !acknowledged`, we attach a `beforeunload` handler so
 * an accidental tab close / reload prompts the browser's standard
 * "leave site?" dialog. This is the ONLY place in the console that
 * uses `beforeunload`.
 * 3. Closing the dialog via the X button or Escape is a no-op until the
 * user clicks "I've saved it" — at that point we call `onAck`, and
 * subsequent `onOpenChange(false)` events propagate normally.
 *
 * Do NOT add "reveal" / "hide" toggles, "regenerate" buttons, or any
 * persistence knob. The anti-features list (console spec) is
 * explicit: this is a one-shot display.
 */
export type OneTimeKeyModalProps = {
  open: boolean
  plaintext: string
  scope: "read" | "write" | "readwrite" | "admin"
  onAck: () => void
}

// Human-readable capability line per scope, so the modal never mislabels a
// key's power (a read key is download-only, not "write").
const SCOPE_BLURB: Record<OneTimeKeyModalProps["scope"], string> = {
  read: "it can download objects on your behalf",
  write: "it can upload new xorbs and shards on your behalf",
  readwrite: "it can upload and download on your behalf — enough for a full round-trip",
  admin: "it has full admin access on your behalf",
}

export function OneTimeKeyModal({ open, plaintext, scope, onAck }: OneTimeKeyModalProps) {
  const [acknowledged, setAcknowledged] = useState(false)

  // Reset the ack state whenever the modal is re-opened with a new key.
  // This matches the expected page-load lifecycle: a fresh /onboarding
  // visit should always start un-ack'd.
  useEffect(() => {
    if (!open) setAcknowledged(false)
  }, [open])

  // beforeunload guard — fires ONLY while modal is open AND un-ack'd.
  // The string returned/assigned to `returnValue` is ignored by modern
  // browsers (they render their own generic message), but setting it to
  // a non-empty value is what actually triggers the prompt.
  useEffect(() => {
    if (!open || acknowledged) return
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault()
      // Legacy browsers read `returnValue`; modern browsers just need
      // `preventDefault` and will render their own text.
      e.returnValue = "Copy your API key first — it cannot be retrieved later."
    }
    window.addEventListener("beforeunload", handler)
    return () => window.removeEventListener("beforeunload", handler)
  }, [open, acknowledged])

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(plaintext)
      toast.success("Copied. Store it securely.")
    } catch {
      toast.error("Clipboard unavailable — select the key manually")
    }
  }

  const ack = () => {
    setAcknowledged(true)
    onAck()
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // Only allow the dialog to close once the user has explicitly
        // acknowledged that they've saved the key. Until then, X / Esc /
        // overlay-click are all no-ops.
        if (next) return
        if (acknowledged) onAck()
      }}
    >
      <DialogContent data-testid="one-time-key-modal" showCloseButton={acknowledged}>
        <DialogHeader>
          <DialogTitle>Your API key (shown once)</DialogTitle>
          <DialogDescription>
            OpenWeights will never display this key again. Store it in a password manager or in your
            project&apos;s <code>.env</code> file before closing this dialog.
          </DialogDescription>
        </DialogHeader>

        <div
          data-testid="one-time-key-plaintext"
          className="flex items-start gap-2 rounded-none border bg-muted p-3 font-mono text-xs/relaxed break-all"
        >
          <WarningCircleIcon className="mt-0.5 size-4 shrink-0 text-foreground" />
          <span className="flex-1">{plaintext}</span>
        </div>

        <p className="text-xs text-muted-foreground">
          This key has <strong>{scope}</strong> scope — {SCOPE_BLURB[scope]}. Treat it like a
          password.
        </p>

        <DialogFooter className="gap-2">
          <Button variant="outline" onClick={copy} data-testid="one-time-key-copy">
            Copy key
          </Button>
          <Button onClick={ack} data-testid="one-time-key-ack">
            I&apos;ve saved it
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

import { ArrowRightIcon, KeyIcon, SparkleIcon } from "@phosphor-icons/react"
import { useNavigate } from "@tanstack/react-router"
import { useEffect, useRef, useState } from "react"

import { CopyPasteCard } from "@/components/CopyPasteCard"
import { OneTimeKeyModal } from "@/components/OneTimeKeyModal"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { type CreatedKey, useCreateKey } from "@/hooks/useKeys"
import { CAS_URL } from "@/lib/api"

/**
 * `/onboarding` page ( + + ).
 *
 * On mount, issues exactly one `POST /admin/keys {name:"onboarding",
 * scope:"write"}` and renders the returned plaintext in:
 *
 * (a) the `<OneTimeKeyModal>` (: beforeunload-guarded one-shot), AND
 * (b) the `HF_XET_DATA_CUSTOM_HEADERS` line inside the first
 * `<CopyPasteCard>`.
 *
 * The plaintext lives in `useState` scoped to this component — React
 * unmounts it on navigation away, at which point the plaintext is
 * garbage-collected. It is never written to `localStorage`,
 * `sessionStorage`, analytics (we have none; see anti-features in
 * .md), or any other persistent sink.
 *
 * Strict-mode double-invoke of the mount effect is guarded with a ref so
 * we never fire two `POST /admin/keys` requests during development.*/

function buildEnvBlock(plaintextKey: string): string {
  return `export HF_XET_DATA_DEFAULT_CAS_ENDPOINT="${CAS_URL}"
export HF_XET_DATA_CUSTOM_HEADERS='{"Authorization":"Bearer ${plaintextKey}"}'`
}

const UPLOAD_EXAMPLE = "huggingface-cli upload <your-username>/siahub-test-repo ./test.bin"

export function OnboardingPage() {
  const [created, setCreated] = useState<CreatedKey | null>(null)
  const [modalOpen, setModalOpen] = useState(false)
  const hasRequested = useRef(false)
  const create = useCreateKey()
  const navigate = useNavigate()

  // : auto-generate exactly one write-scope key on mount.
  // `useRef` guard is redundant with the React Query `isPending` check but
  // belt-and-braces against StrictMode double-invoke in dev.
  useEffect(() => {
    if (hasRequested.current) return
    if (created) return
    hasRequested.current = true
    create.mutate(
      { name: "onboarding", scope: "write" },
      {
        onSuccess: (key) => {
          setCreated(key)
          setModalOpen(true)
        },
        onError: () => {
          // Let the user retry manually — resetting the ref unlocks the
          // "Try again" button below.
          hasRequested.current = false
        },
      },
    )
  }, [create, created])

  // Pending — no key yet, no plaintext to render.
  if (create.isPending || (!created && !create.isError)) {
    return (
      <main className="mx-auto flex min-h-svh max-w-3xl flex-col items-center justify-center px-6 py-10">
        <p className="text-sm text-muted-foreground" data-testid="onboarding-pending">
          Preparing your API key…
        </p>
      </main>
    )
  }

  // Error path — retry CTA that re-arms the mount effect.
  if (create.isError || !created) {
    return (
      <main className="mx-auto max-w-3xl px-6 py-10">
        <Alert variant="destructive">
          <AlertTitle>Could not create your API key</AlertTitle>
          <AlertDescription>
            {create.error?.message ?? "Unknown error"} — try again, or contact your SiaHub operator
            if this persists.
          </AlertDescription>
        </Alert>
        <div className="mt-4">
          <Button
            onClick={() => {
              hasRequested.current = false
              create.reset()
            }}
            data-testid="onboarding-retry"
          >
            Try again
          </Button>
        </div>
      </main>
    )
  }

  const envBlock = buildEnvBlock(created.plaintext_key)

  return (
    <main className="mx-auto max-w-3xl px-6 py-10">
      <div className="flex flex-col gap-6">
        <header className="flex flex-col gap-3">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <SparkleIcon className="size-4" />
            <span>Welcome to SiaHub</span>
          </div>
          <h1 className="font-heading text-3xl font-medium tracking-tight">
            You&apos;re ready to upload to Sia.
          </h1>
          <p className="text-sm text-muted-foreground">
            A fresh <strong>write</strong>-scope API key has been created for you. Paste the two
            export lines into your shell, then run a test upload with <code>huggingface-cli</code>.
          </p>
        </header>

        <Alert data-testid="onboarding-keys-warning">
          <KeyIcon />
          <AlertTitle>Copy your API key now</AlertTitle>
          <AlertDescription>
            The plaintext key is shown once in the dialog and inside the first copy-paste block
            below. Once you leave this page, <strong>this key will never be shown again</strong>.
          </AlertDescription>
        </Alert>

        <CopyPasteCard
          title="1. Set Xet env vars"
          body={envBlock}
          hint="These redirect huggingface-cli to SiaHub instead of Hugging Face's default S3 + CloudFront backend."
          testId="onboarding-env-block"
        />
        <CopyPasteCard
          title="2. Run a test upload"
          body={UPLOAD_EXAMPLE}
          hint="Replace <your-username>/siahub-test-repo with a repo you own on Hugging Face."
          testId="onboarding-upload-block"
        />

        <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          <Button
            variant="outline"
            onClick={() => navigate({ to: "/dashboard" })}
            data-testid="onboarding-skip"
          >
            Skip for now
          </Button>
          <Button
            onClick={() => navigate({ to: "/dashboard" })}
            data-testid="onboarding-to-dashboard"
          >
            Go to dashboard
            <ArrowRightIcon data-icon="inline-end" />
          </Button>
        </div>
      </div>

      <OneTimeKeyModal
        open={modalOpen}
        plaintext={created.plaintext_key}
        onAck={() => setModalOpen(false)}
      />
    </main>
  )
}

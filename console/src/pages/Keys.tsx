import { KeyIcon, WarningCircleIcon } from "@phosphor-icons/react"
import { useState } from "react"
import { toast } from "sonner"

import { CopyPasteCard } from "@/components/CopyPasteCard"
import { OneTimeKeyModal } from "@/components/OneTimeKeyModal"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { CAS_URL } from "@/lib/api"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import {
  type CreatedKey,
  type KeyScope,
  useCreateKey,
  useKeys,
  useRevokeKey,
} from "@/hooks/useKeys"

/**
 * `/keys` page — formerly split across `/onboarding` + `/keys`, now a
 * single "API keys + usage" surface. Four concerns on one page:
 *
 * 1. **Create** — inline form; on success the plaintext lands in
 * `<OneTimeKeyModal>`. The plaintext is shown *exactly once*; if
 * the user dismisses the modal without copying it, they have to
 * mint a new key.
 * 2. **List** (`GET /admin/keys`) — masked prefixes only (
 * invariant 4). Empty state prompts to create the first key.
 * 3. **Usage snippets** — permanent copy-paste blocks for
 * `hf upload` / `hf download` against this OpenWeights deployment,
 * with placeholder `<your-key>` replaced by the freshly minted
 * plaintext when a key was just created this session.
 * 4. **Revoke** (`DELETE /admin/keys/{id}`) — row-level, behind an
 * `<AlertDialog>` confirmation.
 *
 * Error surfacing: list-query errors render a destructive
 * alert above the table; create/revoke errors surface as Sonner toasts.
 * `AuthGuard` handles the 401 → `/login` path upstream.*/

/**
 * Format an ISO-8601 timestamp (or `null`) for a table cell. Keeping the
 * date-renderer in one place makes it trivial to swap in a relative-time
 * library ( nice-to-have) without touching JSX.*/
function formatTs(ts: string | null): string {
  if (!ts) return "—"
  // `toLocaleString` is deterministic in Node's ICU-enabled test env; the
  // tests assert on the input row presence, not on the formatted string.
  return new Date(ts).toLocaleString()
}

export function KeysPage() {
  const list = useKeys()
  const create = useCreateKey()
  const revoke = useRevokeKey()

  const [created, setCreated] = useState<CreatedKey | null>(null)
  const [name, setName] = useState("")
  const [scope, setScope] = useState<KeyScope>("write")

  const canSubmit = name.trim().length > 0 && !create.isPending

  const submit = () => {
    if (!canSubmit) return
    create.mutate(
      { name: name.trim(), scope },
      {
        onSuccess: (key) => {
          setCreated(key)
          setName("")
        },
        onError: (err) => {
          // 409 etc. — let the user re-type. Sonner toast is sufficient;
          // inline field-level validation is out of scope.
          toast.error(`Could not create key: ${err.message}`)
        },
      },
    )
  }

  const doRevoke = (id: string, displayName: string) => {
    revoke.mutate(id, {
      onSuccess: () => {
        toast.success(`Revoked "${displayName}"`)
      },
      onError: (err) => {
        toast.error(`Could not revoke: ${err.message}`)
      },
    })
  }

  const keys = list.data ?? []

  // The usage snippets always show a `<your-openweights-key>` placeholder.
  // Plaintext keys are shown ONCE in the CreatedKeyModal at mint time
  // (line 293) and are never reflected back into the keys list — copy
  // the plaintext from the modal and paste it into your shell. The list
  // endpoint only ever returns the masked prefix.
  const keyPlaceholder = "<your-openweights-key>"
  const uploadSnippet = `HF_TOKEN=${keyPlaceholder} HF_ENDPOINT=${CAS_URL} \\
  hf upload <your-username>/<repo-name> ./your-files`
  const downloadSnippet = `HF_ENDPOINT=${CAS_URL} hf download <your-username>/<repo-name>`

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 py-8">
      <header>
        <div className="flex items-center gap-3">
          <KeyIcon size={22} weight="light" className="text-muted-foreground" />
          <h1 className="font-heading text-2xl font-semibold tracking-tight">
            API keys
          </h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">
          Authenticate uploads and downloads. Plaintext is shown once on
          creation.
        </p>
      </header>

      <div className="flex flex-col gap-6">
        <section
          data-testid="keys-create-section"
          className="flex flex-col gap-3 rounded border bg-muted/10 p-4"
        >
          <div className="flex flex-col gap-1">
            <h2 className="font-heading text-sm font-medium">Create a new key</h2>
            <p className="text-xs text-muted-foreground">
              Name it so you remember. Plaintext appears once — copy it.
            </p>
          </div>
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-1 flex-col gap-1.5">
              <Label htmlFor="keys-name">Name</Label>
              <Input
                id="keys-name"
                data-testid="keys-create-name"
                value={name}
                placeholder="laptop-1"
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") submit()
                }}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="keys-scope">Scope</Label>
              <Select value={scope} onValueChange={(v) => setScope(v as KeyScope)}>
                <SelectTrigger id="keys-scope" data-testid="keys-create-scope" className="min-w-28">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="read">read</SelectItem>
                  <SelectItem value="write">write</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <Button onClick={submit} disabled={!canSubmit} data-testid="keys-create-submit">
              {create.isPending ? "Creating…" : "Create key"}
            </Button>
          </div>
        </section>

        {list.isError && (
          <Alert variant="destructive" data-testid="keys-list-error">
            <WarningCircleIcon />
            <AlertTitle>Could not load your keys</AlertTitle>
            <AlertDescription>
              {list.error?.message ?? "Unknown error"}
            </AlertDescription>
          </Alert>
        )}

        <section className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <h2 className="font-heading text-sm font-medium">Use your key with hf CLI</h2>
            <p className="text-xs text-muted-foreground">
              Prefix <code>hf</code> with two env vars per command.
            </p>
          </div>
          <CopyPasteCard
            title="Upload to OpenWeights"
            body={uploadSnippet}
            hint={`Replace ${keyPlaceholder} with your plaintext key.`}
            testId="keys-upload-snippet"
          />
          <CopyPasteCard
            title="Download from OpenWeights"
            body={downloadSnippet}
            hint="No HF_TOKEN needed for public repos."
            testId="keys-download-snippet"
          />
        </section>

        <section className="flex flex-col gap-2">
          <h2 className="font-heading text-sm font-medium">Active keys</h2>
          <Table data-testid="keys-table">
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Scope</TableHead>
                <TableHead>Prefix</TableHead>
                <TableHead>Created</TableHead>
                <TableHead>Last used</TableHead>
                <TableHead aria-label="Actions" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {list.isPending && (
                <TableRow>
                  <TableCell colSpan={6} className="text-muted-foreground">
                    Loading…
                  </TableCell>
                </TableRow>
              )}
              {!list.isPending && !list.isError && keys.length === 0 && (
                <TableRow data-testid="keys-empty">
                  <TableCell colSpan={6} className="text-muted-foreground">
                    No keys yet — create one to start uploading.
                  </TableCell>
                </TableRow>
              )}
              {keys.map((k) => (
                <TableRow key={k.id} data-testid={`keys-row-${k.id}`}>
                  <TableCell className="font-medium">{k.name}</TableCell>
                  <TableCell>
                    <code>{k.scope}</code>
                  </TableCell>
                  <TableCell>
                    <code data-testid={`keys-prefix-${k.id}`}>{k.masked_prefix}</code>
                  </TableCell>
                  <TableCell>{formatTs(k.created_at)}</TableCell>
                  <TableCell>{formatTs(k.last_used_at)}</TableCell>
                  <TableCell className="text-right">
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <Button variant="ghost" size="sm" data-testid={`keys-revoke-${k.id}`}>
                          Revoke
                        </Button>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>Revoke "{k.name}"?</AlertDialogTitle>
                          <AlertDialogDescription>
                            Requests using this key will start failing with HTTP 401 within 5
                            seconds. This cannot be undone — create a new key if you need to restore
                            access.
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction
                            variant="destructive"
                            onClick={() => doRevoke(k.id, k.name)}
                            data-testid={`keys-revoke-confirm-${k.id}`}
                          >
                            Revoke key
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </section>
      </div>

      {created && (
        <OneTimeKeyModal
          open={!!created}
          plaintext={created.plaintext_key}
          onAck={() => setCreated(null)}
        />
      )}
    </main>
  )
}

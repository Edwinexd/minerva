import { useQuery, useMutation, useQueryClient, queryOptions } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import React from "react"

import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import {
  modelsQuery,
  adminEmbeddingModelsQuery,
  adminRerankerModelsQuery,
  type AdminEmbeddingModel,
  type AdminRerankerModel,
} from "@/lib/queries"
import { RERANKER_MODEL_DISPLAY } from "@/lib/reranker-models"
import { ModelCatalogCard } from "./model-catalog-card"
import { ChatModelsCard } from "./chat-models-card"
import { RelativeTime } from "@/components/relative-time"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Checkbox } from "@/components/ui/checkbox"
import { Skeleton } from "@/components/ui/skeleton"
import { Textarea } from "@/components/ui/textarea"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Badge } from "@/components/ui/badge"

// Mirrors `crate::system_defaults::KnobKind` on the server. The
// frontend uses `type` to pick the right widget; the inner fields
// drive validation hints and the dropdown source.
type KnobKind =
  | { type: "bool" }
  | { type: "int"; min: number; max: number }
  | { type: "float"; min: number; max: number }
  | { type: "text"; multiline: boolean; max_len: number; nullable: boolean }
  | { type: "enum"; options: string[] }
  | { type: "chat_model" }

interface SystemDefaultEntry {
  key: string
  category: "course_ai" | "platform"
  label_key: string
  description_key: string
  kind: KnobKind
  env_var: string | null
  fallback: unknown
  value: unknown
  has_row: boolean
  updated_at: string | null
}

interface SystemDefaultsResponse {
  defaults: SystemDefaultEntry[]
}

const systemDefaultsQuery = queryOptions({
  queryKey: ["admin", "system-defaults"],
  queryFn: () => api.get<SystemDefaultsResponse>("/admin/system-defaults"),
  staleTime: 5_000,
})

/// Format a JSON value for display in the fallback/current-value
/// hint. Booleans render as their word, null as `null`, strings
/// quoted so the admin can see whitespace. Numbers print compactly.
function formatJsonValue(v: unknown): string {
  if (v === null || v === undefined) return "null"
  if (typeof v === "string") return v === "" ? '""' : v
  if (typeof v === "boolean") return v ? "true" : "false"
  if (typeof v === "number") return String(v)
  return JSON.stringify(v)
}

export function AdminDefaultsPanel() {
  const { t } = useTranslation("admin")
  const { t: tCommon } = useTranslation("common")
  const formatError = useApiErrorMessage()
  const { data, isLoading, error } = useQuery(systemDefaultsQuery)

  if (isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    )
  }

  if (error || !data) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("defaults.title")}</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-destructive">
            {error ? formatError(error) : t("defaults.errors.loadFailed")}
          </p>
        </CardContent>
      </Card>
    )
  }

  const courseAi = data.defaults.filter((d) => d.category === "course_ai")
  const platform = data.defaults.filter((d) => d.category === "platform")

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("defaults.title")}</CardTitle>
          <CardDescription>{t("defaults.description")}</CardDescription>
        </CardHeader>
      </Card>

      <CategoryCard
        title={t("defaults.categories.course_ai")}
        entries={courseAi}
        tCommon={tCommon}
      />
      <CategoryCard
        title={t("defaults.categories.platform")}
        entries={platform}
        tCommon={tCommon}
      />

      {/* Model catalogs: which embedding / re-ranker models teachers can
          pick, and which one new courses default to. Live here (not the
          System tab) because they govern new-course defaults. */}
      <ChatModelsCard />
      <EmbeddingModelsCard />
      <RerankerModelsCard />
    </div>
  )
}

function CategoryCard({
  title,
  entries,
  tCommon,
}: {
  title: string
  entries: SystemDefaultEntry[]
  tCommon: (k: string) => string
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="space-y-6">
          {entries.map((entry) => (
            // Keying on `updated_at + has_row` makes the row remount
            // (and so reset its local draft state) only when the
            // server's view of this knob actually changed. Pure
            // query refetches that return the same value keep the
            // existing draft alive, so the admin doesn't lose
            // mid-edit text just because the polling interval ticked.
            <DefaultRow
              key={`${entry.key}::${entry.updated_at ?? "fallback"}::${entry.has_row}`}
              entry={entry}
              tCommon={tCommon}
            />
          ))}
        </div>
      </CardContent>
    </Card>
  )
}

function DefaultRow({
  entry,
  tCommon,
}: {
  entry: SystemDefaultEntry
  tCommon: (k: string) => string
}) {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const formatError = useApiErrorMessage()

  // Local draft state for the editor. Initialized from the server's
  // current value. We don't sync this back from props on every
  // refetch ; the row's `key` includes `updated_at`/`has_row` so a
  // genuine server-side change unmounts/remounts and we get a fresh
  // initial value here. That avoids the `setState`-in-`useEffect`
  // anti-pattern while still snapping the editor to the canonical
  // form after a successful save or a "Reset to fallback".
  const [draft, setDraft] = React.useState<unknown>(entry.value)
  const [serverValueAtMount] = React.useState<unknown>(entry.value)

  const [savedFlash, setSavedFlash] = React.useState(false)

  const saveMutation = useMutation({
    mutationFn: (value: unknown) =>
      api.put<{ key: string; value: unknown; updated_at: string }>(
        "/admin/system-defaults",
        { key: entry.key, value },
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "system-defaults"] })
      setSavedFlash(true)
      window.setTimeout(() => setSavedFlash(false), 2_000)
    },
  })

  const resetMutation = useMutation({
    mutationFn: () =>
      api.delete<{ removed: boolean }>(
        `/admin/system-defaults/${encodeURIComponent(entry.key)}`,
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "system-defaults"] })
    },
  })

  const dirty = !shallowJsonEqual(draft, entry.value)

  return (
    <div className="space-y-2 border-b pb-6 last:border-b-0 last:pb-0">
      <div className="flex items-baseline justify-between gap-2">
        <Label htmlFor={`default-${entry.key}`} className="font-medium">
          {t(entry.label_key)}
        </Label>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          {entry.has_row ? (
            <Badge variant="outline">{t("defaults.rowState.set")}</Badge>
          ) : (
            <Badge variant="secondary">{t("defaults.rowState.fallback")}</Badge>
          )}
          {entry.updated_at && (
            <span>
              {t("defaults.rowState.updatedAtPrefix")}{" "}
              <RelativeTime date={entry.updated_at} />
            </span>
          )}
        </div>
      </div>

      <p className="text-xs text-muted-foreground">{t(entry.description_key)}</p>

      <ValueEditor
        id={`default-${entry.key}`}
        kind={entry.kind}
        value={draft}
        onChange={setDraft}
      />

      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <div className="space-x-3">
          <span>
            {t("defaults.fallbackHint", { value: formatJsonValue(entry.fallback) })}
          </span>
          {entry.env_var && (
            <span>
              {t("defaults.envVarHint", { name: entry.env_var })}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {savedFlash && (
            <span className="text-xs text-emerald-600">
              {t("defaults.savedFlash")}
            </span>
          )}
          {entry.has_row && (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              title={t("defaults.resetTooltip")}
              onClick={() => resetMutation.mutate()}
              disabled={resetMutation.isPending || saveMutation.isPending}
            >
              {t("defaults.resetButton")}
            </Button>
          )}
          <Button
            type="button"
            size="sm"
            onClick={() => saveMutation.mutate(draft)}
            disabled={
              !dirty || saveMutation.isPending || resetMutation.isPending
            }
          >
            {saveMutation.isPending
              ? tCommon("actions.saving")
              : t("defaults.saveButton")}
          </Button>
        </div>
      </div>

      {saveMutation.error && (
        <p className="text-xs text-destructive">
          {t("defaults.errors.saveFailed", {
            detail: formatError(saveMutation.error),
          })}
        </p>
      )}
      {resetMutation.error && (
        <p className="text-xs text-destructive">
          {t("defaults.errors.resetFailed", {
            detail: formatError(resetMutation.error),
          })}
        </p>
      )}

      {/* Hidden so we keep a stable React-Query refresh anchor:
          the mount snapshot lets us tell the user "this is what the
          server thought when the page loaded" if they want to compare. */}
      <input type="hidden" value={formatJsonValue(serverValueAtMount)} />
    </div>
  )
}

/// Render the right widget for each `KnobKind`. Number inputs clamp
/// in the validation hint (`min`/`max` attrs), enum/chat_model use
/// the existing Select widget, text uses Input/Textarea, bool uses
/// the existing Checkbox.
function ValueEditor({
  id,
  kind,
  value,
  onChange,
}: {
  id: string
  kind: KnobKind
  value: unknown
  onChange: (v: unknown) => void
}) {
  // Chat-model picker: source the dropdown from the live Cerebras
  // catalog (same feed the teacher config page uses). Free-text
  // fallback if the API failed; an admin should still be able to
  // pin a model that's temporarily missing from the listing.
  const { data: modelsData } = useQuery({
    ...modelsQuery,
    enabled: kind.type === "chat_model",
  })

  if (kind.type === "bool") {
    return (
      <div className="flex items-center gap-2">
        <Checkbox
          id={id}
          checked={Boolean(value)}
          onCheckedChange={(v) => onChange(Boolean(v))}
        />
        <Label htmlFor={id} className="text-sm">
          {value ? "true" : "false"}
        </Label>
      </div>
    )
  }

  if (kind.type === "int" || kind.type === "float") {
    return (
      <Input
        id={id}
        type="number"
        inputMode={kind.type === "int" ? "numeric" : "decimal"}
        min={kind.min}
        max={kind.max}
        step={kind.type === "int" ? 1 : 0.01}
        value={value == null ? "" : String(value)}
        onChange={(e) => {
          const raw = e.target.value
          if (raw === "") {
            onChange(null)
            return
          }
          const n = kind.type === "int" ? parseInt(raw, 10) : parseFloat(raw)
          if (Number.isFinite(n)) onChange(n)
        }}
      />
    )
  }

  if (kind.type === "text") {
    const str = typeof value === "string" ? value : value == null ? "" : ""
    if (kind.multiline) {
      return (
        <Textarea
          id={id}
          rows={4}
          maxLength={kind.max_len}
          value={str}
          onChange={(e) => {
            const v = e.target.value
            onChange(kind.nullable && v === "" ? null : v)
          }}
        />
      )
    }
    return (
      <Input
        id={id}
        type="text"
        maxLength={kind.max_len}
        value={str}
        onChange={(e) => {
          const v = e.target.value
          onChange(kind.nullable && v === "" ? null : v)
        }}
      />
    )
  }

  if (kind.type === "enum") {
    const current = typeof value === "string" ? value : ""
    return (
      <Select value={current} onValueChange={(v) => onChange(v)}>
        <SelectTrigger id={id} className="w-full max-w-md">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {kind.options.map((opt) => (
            <SelectItem key={opt} value={opt}>
              {opt}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    )
  }

  if (kind.type === "chat_model") {
    const current = typeof value === "string" ? value : ""
    const apiModels = modelsData?.models ?? []
    // Patch in the current pinned value if Cerebras's listing
    // dropped it (rare, but possible during rollouts) so the
    // admin doesn't see an empty Select for a non-empty stored
    // value.
    const options = apiModels.some((m) => m.id === current)
      ? apiModels
      : current
        ? [{ id: current, name: current }, ...apiModels]
        : apiModels
    return (
      <Select value={current} onValueChange={(v) => v && onChange(v)}>
        <SelectTrigger id={id} className="w-full max-w-md truncate">
          <SelectValue placeholder={current} />
        </SelectTrigger>
        <SelectContent>
          {options.map((m) => (
            <SelectItem key={m.id} value={m.id}>
              {m.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    )
  }

  // Exhaustiveness guard. If a new KnobKind ships from the server
  // without a UI for it, render the raw JSON so the admin at least
  // sees the current value rather than nothing.
  return (
    <code className="block rounded bg-muted px-2 py-1 text-xs">
      {JSON.stringify(value)}
    </code>
  )
}

/// Cheap JSON-shaped equality. Good enough for the default editor:
/// values are primitives or short strings, never deeply nested.
function shallowJsonEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true
  if (a == null || b == null) return a === b
  if (typeof a !== typeof b) return false
  if (typeof a === "object") return JSON.stringify(a) === JSON.stringify(b)
  return false
}

// ── Admin model catalogs ───────────────────────────────────────────
// Both render through the shared `ModelCatalogCard` (one component =
// guaranteed-identical column order + spacing); only the per-catalog
// props differ. They live on the Defaults tab (not System) because they
// govern new-course defaults.

function EmbeddingModelsCard() {
  const { t } = useTranslation("admin")
  const { data, isLoading, error } = useQuery(adminEmbeddingModelsQuery)
  return (
    <ModelCatalogCard<AdminEmbeddingModel>
      i18nPrefix="system.embeddingModels"
      data={data}
      isLoading={isLoading}
      error={error}
      adminQueryKey={["admin", "embedding-models"]}
      pickerQueryKey={["embedding-models"]}
      benchmarkQueryKey={["embedding-benchmarks"]}
      enabledPath="/admin/embedding-models"
      defaultPath="/admin/embedding-models/default"
      benchmarkPath="/admin/embedding-benchmark"
      defaultRadioName="default-embedding-model"
      extraColumns={[{ headerKey: "dimensions", render: (m) => m.dimensions }]}
      speedOf={(m) => (m.benchmark ? m.benchmark.embeddings_per_second : null)}
      renderModelName={(m) => (
        <>
          {m.model}
          {m.warmed_at_startup && (
            <Badge variant="secondary" className="ml-2">
              {t("system.embeddingModels.warmBadge")}
            </Badge>
          )}
        </>
      )}
    />
  )
}

function RerankerModelsCard() {
  const { t } = useTranslation("admin")
  const { data, isLoading, error } = useQuery(adminRerankerModelsQuery)
  return (
    <ModelCatalogCard<AdminRerankerModel>
      i18nPrefix="system.rerankerModels"
      data={data}
      isLoading={isLoading}
      error={error}
      adminQueryKey={["admin", "reranker-models"]}
      pickerQueryKey={["reranker-models"]}
      enabledPath="/admin/reranker-models"
      defaultPath="/admin/reranker-models/default"
      benchmarkPath="/admin/reranker-benchmark"
      defaultRadioName="default-reranker-model"
      speedOf={(m) => (m.benchmark ? m.benchmark.pairs_per_second : null)}
      renderModelName={(m) => {
        const meta = RERANKER_MODEL_DISPLAY[m.model]
        return (
          <>
            {meta?.name ?? m.model}
            {meta?.multilingual && (
              <Badge variant="secondary" className="ml-2">
                {t("system.rerankerModels.multilingualBadge")}
              </Badge>
            )}
          </>
        )
      }}
    />
  )
}

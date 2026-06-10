import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { useState } from "react"

import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { adminChatModelsQuery, type AdminChatModel } from "@/lib/queries"
import { chatModelDisplayName } from "@/lib/chat-models"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Skeleton } from "@/components/ui/skeleton"
import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"

const ADMIN_KEY = ["admin", "chat-models"] as const
const PICKER_KEY = ["chat-models"] as const

/// Price suggestion returned by the scrape endpoint. Never persisted by
/// the server; the admin reviews and saves via the price PUT.
interface PriceSuggestion {
  input_usd_per_mtok: number | null
  output_usd_per_mtok: number | null
  confidence: number | null
  note: string | null
  source_url: string
  page_fetched: boolean
}

/// Admin catalog of chat / utility models: enable, set the course-chat
/// default + the utility default, edit per-model USD prices, and a
/// best-effort "scrape price" helper. Enabling is gated on a known price
/// (both rates entered; 0 is allowed) and a configured provider key.
export function ChatModelsCard() {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const { data, isLoading, error } = useQuery(adminChatModelsQuery)

  const refreshMut = useMutation({
    mutationFn: () => api.post("/admin/chat-models/refresh", {}),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ADMIN_KEY })
      queryClient.invalidateQueries({ queryKey: PICKER_KEY })
    },
  })

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between gap-2">
          <CardTitle>{t("system.chatModels.title")}</CardTitle>
          <Button
            size="sm"
            variant="outline"
            disabled={refreshMut.isPending}
            onClick={() => refreshMut.mutate()}
          >
            {refreshMut.isPending
              ? t("system.chatModels.refreshing")
              : t("system.chatModels.refresh")}
          </Button>
        </div>
        <CardDescription>{t("system.chatModels.description")}</CardDescription>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <div className="space-y-2">
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-full" />
          </div>
        ) : error ? (
          <p className="text-sm text-destructive">
            {t("system.chatModels.loadError")}
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b text-left text-xs text-muted-foreground">
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.enabled")}
                  </th>
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.model")}
                  </th>
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.default")}
                  </th>
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.utility")}
                  </th>
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.inputPrice")}
                  </th>
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.outputPrice")}
                  </th>
                  <th scope="col" className="py-2 pr-3">
                    {t("system.chatModels.col.courses")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {data?.models.map((m) => (
                  <ChatModelRow key={m.model} model={m} />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function ChatModelRow({ model }: { model: AdminChatModel }) {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const formatError = useApiErrorMessage()

  const [inputDraft, setInputDraft] = useState(
    model.input_usd_per_mtok === null ? "" : String(model.input_usd_per_mtok),
  )
  const [outputDraft, setOutputDraft] = useState(
    model.output_usd_per_mtok === null ? "" : String(model.output_usd_per_mtok),
  )
  const [scrapeNote, setScrapeNote] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ADMIN_KEY })
    queryClient.invalidateQueries({ queryKey: PICKER_KEY })
  }
  const onError = (e: unknown) => setActionError(formatError(e))

  const enableMut = useMutation({
    mutationFn: (enabled: boolean) =>
      api.put("/admin/chat-models", { model: model.model, enabled }),
    onSuccess: invalidate,
    onError,
  })
  const defaultMut = useMutation({
    mutationFn: () =>
      api.put("/admin/chat-models/default", { model: model.model }),
    onSuccess: invalidate,
    onError,
  })
  const utilityMut = useMutation({
    mutationFn: () =>
      api.put("/admin/chat-models/utility-default", { model: model.model }),
    onSuccess: invalidate,
    onError,
  })
  const priceMut = useMutation({
    mutationFn: (rates: { input: number; output: number }) =>
      api.put("/admin/chat-models/price", {
        model: model.model,
        input_usd_per_mtok: rates.input,
        output_usd_per_mtok: rates.output,
      }),
    onSuccess: () => {
      setScrapeNote(null)
      invalidate()
    },
    onError,
  })
  const scrapeMut = useMutation({
    mutationFn: () =>
      api.post<PriceSuggestion>(
        `/admin/chat-models/${encodeURIComponent(model.model)}/scrape-price`,
        {},
      ),
    onSuccess: (s) => {
      if (s.input_usd_per_mtok !== null) setInputDraft(String(s.input_usd_per_mtok))
      if (s.output_usd_per_mtok !== null)
        setOutputDraft(String(s.output_usd_per_mtok))
      setScrapeNote(
        s.note ?? t("system.chatModels.scrapeDone", { url: s.source_url }),
      )
    },
    onError,
  })

  const unpriced =
    model.input_usd_per_mtok === null || model.output_usd_per_mtok === null
  const canEnable = model.provider_available && !unpriced
  const priceDirty =
    inputDraft !== (model.input_usd_per_mtok === null ? "" : String(model.input_usd_per_mtok)) ||
    outputDraft !== (model.output_usd_per_mtok === null ? "" : String(model.output_usd_per_mtok))
  const pricesValid =
    inputDraft.trim() !== "" &&
    outputDraft.trim() !== "" &&
    Number(inputDraft) >= 0 &&
    Number(outputDraft) >= 0

  return (
    <tr className="border-b align-top">
      <td className="py-2 pr-3">
        <Checkbox
          checked={model.enabled}
          disabled={!canEnable || enableMut.isPending}
          onCheckedChange={(c) => enableMut.mutate(c === true)}
          aria-label={t("system.chatModels.enableLabel", {
            model: model.display_name,
          })}
        />
      </td>
      <td className="py-2 pr-3">
        <div className="font-medium">{chatModelDisplayName(model)}</div>
        <div className="mt-1 flex flex-wrap gap-1">
          <Badge variant="secondary">{model.provider}</Badge>
          {unpriced && (
            <Badge variant="outline">{t("system.chatModels.unpriced")}</Badge>
          )}
          {!model.provider_available && (
            <Badge variant="destructive">
              {t("system.chatModels.providerUnavailable")}
            </Badge>
          )}
        </div>
        {scrapeNote && (
          <p className="mt-1 text-xs text-muted-foreground">{scrapeNote}</p>
        )}
        {actionError && (
          <p className="mt-1 text-xs text-destructive">{actionError}</p>
        )}
      </td>
      <td className="py-2 pr-3">
        <input
          type="radio"
          name="chat-default-model"
          checked={model.is_default}
          disabled={!model.enabled || defaultMut.isPending}
          onChange={() => defaultMut.mutate()}
          aria-label={t("system.chatModels.defaultLabel", {
            model: model.display_name,
          })}
        />
      </td>
      <td className="py-2 pr-3">
        <input
          type="radio"
          name="chat-utility-model"
          checked={model.is_utility_default}
          disabled={!model.enabled || utilityMut.isPending}
          onChange={() => utilityMut.mutate()}
          aria-label={t("system.chatModels.utilityLabel", {
            model: model.display_name,
          })}
        />
      </td>
      <td className="py-2 pr-3">
        <Input
          type="number"
          step="0.000001"
          min={0}
          className="h-7 w-24 text-xs"
          value={inputDraft}
          placeholder={t("system.chatModels.unpriced")}
          onChange={(e) => setInputDraft(e.target.value)}
          aria-label={t("system.chatModels.inputPriceLabel", {
            model: model.display_name,
          })}
        />
      </td>
      <td className="py-2 pr-3">
        <Input
          type="number"
          step="0.000001"
          min={0}
          className="h-7 w-24 text-xs"
          value={outputDraft}
          placeholder={t("system.chatModels.unpriced")}
          onChange={(e) => setOutputDraft(e.target.value)}
          aria-label={t("system.chatModels.outputPriceLabel", {
            model: model.display_name,
          })}
        />
        <div className="mt-1 flex gap-1">
          {priceDirty && pricesValid && (
            <Button
              size="sm"
              variant="outline"
              className="h-6 text-xs"
              disabled={priceMut.isPending}
              onClick={() =>
                priceMut.mutate({
                  input: Number(inputDraft),
                  output: Number(outputDraft),
                })
              }
            >
              {t("system.chatModels.savePrice")}
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            className="h-6 text-xs"
            disabled={scrapeMut.isPending}
            onClick={() => {
              setActionError(null)
              scrapeMut.mutate()
            }}
          >
            {scrapeMut.isPending
              ? t("system.chatModels.scraping")
              : t("system.chatModels.scrape")}
          </Button>
        </div>
      </td>
      <td className="py-2 pr-3 text-muted-foreground">{model.courses_using}</td>
    </tr>
  )
}

import { useState } from "react"
import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import ReactMarkdown from "react-markdown"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Card } from "@/components/ui/card"
import { useApiErrorMessage } from "@/lib/use-api-error"

/**
 * First step of the study pipeline. Renders researcher-supplied consent
 * copy (markdown source stored on `study_courses.consent_html`; column
 * name predates the markdown decision) and gates progression on an
 * explicit checkbox + button. Both are required server-side too: the
 * checkbox value is sent as `consent_given: true` in the POST body, and
 * the route refuses if it's not literally true.
 */
export function ConsentScreen({
  courseId,
  consentMarkdown,
}: {
  courseId: string
  consentMarkdown: string
}) {
  const { t } = useTranslation("study")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [agreed, setAgreed] = useState(false)
  const [error, setError] = useState<unknown>(null)

  const mutation = useMutation({
    mutationFn: () =>
      api.post<{ stage: string }>(`/courses/${courseId}/study/consent`, {
        consent_given: true,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "study", "state"],
      })
    },
    onError: (e) => setError(e),
  })

  return (
    <Card className="mx-auto my-8 max-w-3xl space-y-6 p-6">
      <h1 className="text-2xl font-semibold">{t("consent.title")}</h1>

      <div className="prose prose-sm max-w-none dark:prose-invert">
        {consentMarkdown.trim() === "" ? (
          <p className="text-muted-foreground italic">
            {t("consent.missingHtml")}
          </p>
        ) : (
          <ReactMarkdown>{consentMarkdown}</ReactMarkdown>
        )}
      </div>

      <label className="flex items-start gap-3 text-sm">
        <Checkbox
          checked={agreed}
          onCheckedChange={(v) => setAgreed(v === true)}
          disabled={mutation.isPending}
        />
        <span className="leading-snug">{t("consent.checkboxLabel")}</span>
      </label>

      {error !== null && (
        <p role="alert" className="text-sm text-destructive">
          {formatError(error)}
        </p>
      )}

      <div className="flex justify-end">
        <Button
          onClick={() => mutation.mutate()}
          disabled={!agreed || mutation.isPending}
        >
          {mutation.isPending
            ? t("consent.submittingButton")
            : t("consent.submitButton")}
        </Button>
      </div>
    </Card>
  )
}

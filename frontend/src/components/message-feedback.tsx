import { useState } from "react"
import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { ThumbsDown, ThumbsUp } from "lucide-react"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import { api } from "@/lib/api"
import { FEEDBACK_CATEGORIES, type MessageFeedback } from "@/lib/types"

interface Props {
  courseId: string
  conversationId: string
  messageId: string
  current: MessageFeedback | null
}

export function FeedbackControls({
  courseId,
  conversationId,
  messageId,
  current,
}: Props) {
  const { t } = useTranslation("student")
  const { t: tCommon } = useTranslation("common")
  const queryClient = useQueryClient()
  const [downOpen, setDownOpen] = useState(false)
  const [category, setCategory] = useState<string>("")
  const [comment, setComment] = useState<string>("")

  const setMutation = useMutation({
    mutationFn: (body: { rating: "up" | "down"; category?: string; comment?: string }) =>
      api.put(
        `/courses/${courseId}/conversations/${conversationId}/messages/${messageId}/feedback`,
        body,
      ),
    onSuccess: () => {
      setDownOpen(false)
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations", conversationId],
      })
    },
  })

  const upActive = current?.rating === "up"
  const downActive = current?.rating === "down"
  const busy = setMutation.isPending

  const handleUp = () => {
    // Ratings can't be removed once given; clicking the active rating is a
    // no-op. Switching from down -> up is allowed (no details required).
    if (busy || upActive) return
    setMutation.mutate({ rating: "up" })
  }

  const handleDown = () => {
    if (busy) return
    // Seed the form here (instead of in an effect) so opening the modal is
    // a single render with the right initial values.
    setCategory(downActive ? current?.category ?? "" : "")
    setComment(downActive ? current?.comment ?? "" : "")
    setDownOpen(true)
  }

  const handleSubmitDown = () => {
    if (!category) return
    setMutation.mutate({
      rating: "down",
      category,
      comment: comment || undefined,
    })
  }

  return (
    <>
      <div className="flex items-center gap-1 ml-auto">
        <button
          type="button"
          onClick={handleUp}
          disabled={busy}
          title={upActive ? t("feedback.thumbsUpActiveTitle") : t("feedback.thumbsUpTitle")}
          className={`p-1 rounded hover:bg-foreground/10 disabled:opacity-50 ${
            upActive ? "text-green-600 dark:text-green-400" : ""
          }`}
        >
          <ThumbsUp className="w-3.5 h-3.5" />
        </button>
        <button
          type="button"
          onClick={handleDown}
          disabled={busy}
          title={downActive ? t("feedback.thumbsDownActiveTitle") : t("feedback.thumbsDownTitle")}
          className={`p-1 rounded hover:bg-foreground/10 disabled:opacity-50 ${
            downActive ? "text-red-600 dark:text-red-400" : ""
          }`}
        >
          <ThumbsDown className="w-3.5 h-3.5" />
        </button>
      </div>

      <AlertDialog open={downOpen} onOpenChange={setDownOpen}>
        <AlertDialogContent className="sm:max-w-md">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("feedback.dialogTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("feedback.dialogDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>

          <div className="space-y-3">
            <div className="space-y-1.5">
              <Label htmlFor="feedback-category">
                {t("feedback.categoryLabel")} <span className="text-destructive">{t("feedback.required")}</span>
              </Label>
              <Select value={category} onValueChange={(v) => v && setCategory(v)}>
                <SelectTrigger id="feedback-category" className="w-full">
                  <SelectValue placeholder={t("feedback.categoryPlaceholder")} />
                </SelectTrigger>
                <SelectContent>
                  {FEEDBACK_CATEGORIES.map((c) => (
                    <SelectItem key={c.value} value={c.value}>
                      {t(`feedback.categories.${c.value}`, { defaultValue: c.label })}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="feedback-comment">{t("feedback.commentLabel")}</Label>
              <Textarea
                id="feedback-comment"
                value={comment}
                onChange={(e) => setComment(e.target.value)}
                placeholder={t("feedback.commentPlaceholder")}
                rows={4}
              />
            </div>
          </div>

          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy}>{tCommon("actions.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                // Keep the dialog open if validation fails or the request is
                // still in flight; the mutation's onSuccess closes it.
                e.preventDefault()
                handleSubmitDown()
              }}
              disabled={busy || !category}
            >
              {setMutation.isPending ? t("feedback.submitting") : t("feedback.submit")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

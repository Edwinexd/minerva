import { useEffect, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { ChatWindow } from "@/components/chat/chat-page"
import { useApiErrorMessage } from "@/lib/use-api-error"
import type {
  StudyFinishTaskResponse,
  StudyStartTaskResponse,
} from "@/lib/types"

/**
 * Renders the per-task study chrome (banner + Done button) above a
 * regular ChatWindow pinned to the per-task conversation. The
 * conversation is created lazily on first /task/{i}/start hit and
 * cached server-side, so re-entering the same task slot after a tab
 * close lands back in the same conversation.
 *
 * Aegis support is per-round: the study config (`study_tasks.aegis_enabled`)
 * decides whether this task's chat shows the Aegis panel and runs the
 * live analyzer / rewrite endpoints. The backend's
 * `feature_flags::aegis_enabled_for_conversation` helper enforces the
 * same gate server-side, so a hand-crafted request from the client can't
 * bypass the round's setting.
 */
export function TaskRunner({
  courseId,
  taskIndex,
  totalTasks,
  title,
  description,
  aegisEnabled,
  conversationIdFromState,
}: {
  courseId: string
  taskIndex: number
  totalTasks: number
  title: string
  description: string
  /** Per-round Aegis gate from `StudyState.current_task.aegis_enabled`. */
  aegisEnabled: boolean
  /** May be null if /state ran before the participant ever started this task. */
  conversationIdFromState: string | null
}) {
  const { t } = useTranslation("study")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [submitError, setSubmitError] = useState<unknown>(null)

  // If /state didn't include a conversation (first time landing on
  // this task), kick the start endpoint to materialise it. The
  // result is the same row that subsequent /state calls will hand
  // back, so we just refetch /state to settle.
  const startQuery = useQuery({
    queryKey: ["courses", courseId, "study", "task", taskIndex, "start"],
    queryFn: () =>
      api.post<StudyStartTaskResponse>(
        `/courses/${courseId}/study/task/${taskIndex}/start`,
        {},
      ),
    enabled: conversationIdFromState === null,
    staleTime: Infinity,
  })

  useEffect(() => {
    if (startQuery.data) {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "study", "state"],
      })
    }
  }, [startQuery.data, courseId, queryClient])

  const conversationId =
    conversationIdFromState ?? startQuery.data?.conversation_id ?? null

  const doneMutation = useMutation({
    mutationFn: () =>
      api.post<StudyFinishTaskResponse>(
        `/courses/${courseId}/study/task/${taskIndex}/done`,
        {},
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "study", "state"],
      })
    },
    onError: (e) => setSubmitError(e),
  })

  return (
    <div className="flex h-[calc(100vh-120px)] flex-col gap-4">
      <Card className="space-y-3 p-4">
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1">
            <p className="text-xs uppercase tracking-wide text-muted-foreground">
              {t("task.banner", {
                current: taskIndex + 1,
                total: totalTasks,
              })}
            </p>
            <h2 className="text-lg font-semibold leading-tight">{title}</h2>
          </div>
          <Button
            onClick={() => {
              setSubmitError(null)
              doneMutation.mutate()
            }}
            disabled={doneMutation.isPending}
          >
            {doneMutation.isPending
              ? t("task.doneSubmittingButton")
              : t("task.doneButton")}
          </Button>
        </div>
        <p className="whitespace-pre-wrap text-sm text-foreground/80">
          {description}
        </p>
        {submitError !== null && (
          <p role="alert" className="text-sm text-destructive">
            {formatError(submitError)}
          </p>
        )}
      </Card>

      {/*
        ChatWindow's root is `flex flex-1 min-h-0`; it needs a flex
        column ancestor (this TaskRunner div) to get bounded height
        and to let its inner transcript scroll instead of pushing
        the page height up. Wrapping it in a non-flex div was
        breaking layout: the transcript grew to its full content
        height and the composer + page chrome ended up beneath it.
        Render it as a direct child of the flex column instead.
      */}
      {conversationId === null ? (
        <Skeleton className="flex-1 min-h-0 w-full" />
      ) : (
        <ChatWindow
          courseId={courseId}
          conversationId={conversationId}
          aegisEnabled={aegisEnabled}
          readOnly={false}
          // Pin Aegis to expert calibration for every study
          // participant. Without this, prior localStorage values from
          // a participant's earlier non-study chat use would inject
          // mode variance into the eval data. (Inert on
          // aegisEnabled=false rounds, but cheap to keep set.)
          forceAegisMode="expert"
        />
      )}
    </div>
  )
}

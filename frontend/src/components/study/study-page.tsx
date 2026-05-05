import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { studyStateQuery } from "@/lib/queries"
import { Card } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { ConsentScreen } from "./consent-screen"
import { SurveyForm } from "./survey-form"
import { TaskRunner } from "./task-runner"
import { ThankYouScreen } from "./thank-you-screen"

/**
 * Top-level study dispatcher. Reads `/api/courses/{id}/study/state`
 * and renders the right step based on `stage`. Stages are linear
 * and server-enforced; the frontend never advances them on its own,
 * so a successful POST to `/consent`, `/survey/{kind}`, or
 * `/task/{i}/done` is followed by an /state invalidation that swaps
 * the rendered child component automatically.
 */
export function StudyPage({
  useParams,
}: {
  useParams: () => { courseId: string }
}) {
  const { courseId } = useParams()
  const { t } = useTranslation("study")
  const formatError = useApiErrorMessage()
  const { data: state, isLoading, error } = useQuery(studyStateQuery(courseId))

  if (isLoading) {
    return (
      <Card className="mx-auto my-8 max-w-3xl space-y-3 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-32 w-full" />
        <p className="text-sm text-muted-foreground">{t("loading")}</p>
      </Card>
    )
  }

  if (error || !state) {
    return (
      <Card className="mx-auto my-8 max-w-3xl space-y-2 p-6">
        <h2 className="text-lg font-semibold">{t("loadFailed")}</h2>
        {error !== null && (
          <p role="alert" className="text-sm text-destructive">
            {formatError(error)}
          </p>
        )}
      </Card>
    )
  }

  switch (state.stage) {
    case "consent":
      return (
        <ConsentScreen
          courseId={courseId}
          consentMarkdown={state.consent_html}
        />
      )
    case "pre_survey":
      return <SurveyForm courseId={courseId} kind="pre" />
    case "task":
      if (!state.current_task) {
        return (
          <Card className="mx-auto my-8 max-w-3xl p-6">
            <p className="text-sm text-destructive">{t("task.missingTask")}</p>
          </Card>
        )
      }
      return (
        <TaskRunner
          courseId={courseId}
          taskIndex={state.current_task.task_index}
          totalTasks={state.number_of_tasks}
          title={state.current_task.title}
          description={state.current_task.description}
          conversationIdFromState={state.current_task_conversation_id}
        />
      )
    case "post_survey":
      return <SurveyForm courseId={courseId} kind="post" />
    case "done":
      return <ThankYouScreen thankYouMarkdown={state.thank_you_html} />
  }
}

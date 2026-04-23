import { useNavigate } from "@tanstack/react-router"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { api } from "@/lib/api"
import { Skeleton } from "@/components/ui/skeleton"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { useDocumentTitle } from "@/lib/use-document-title"

export function JoinPage({ useParams }: { useParams: () => { token: string } }) {
  const { t } = useTranslation("auth")
  const { t: tCommon } = useTranslation("common")
  useDocumentTitle(tCommon("pageTitles.join"))
  const { token } = useParams()
  const navigate = useNavigate()
  const formatError = useApiErrorMessage()
  const [error, setError] = useState<unknown>(null)

  useEffect(() => {
    api
      .get<{ joined: boolean; course_id: string }>(`/join/${token}`)
      .then((data) => {
        navigate({
          to: "/course/$courseId",
          params: { courseId: data.course_id },
          replace: true,
        })
      })
      .catch((e) => {
        setError(e)
      })
  }, [token, navigate])

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center py-20 gap-4">
        <p className="text-destructive text-lg">{formatError(error) || t("join.failedToJoin")}</p>
      </div>
    )
  }

  return (
    <div className="flex flex-col items-center justify-center py-20 gap-4">
      <Skeleton className="h-6 w-48" />
      <p className="text-muted-foreground">{t("join.joining")}</p>
    </div>
  )
}

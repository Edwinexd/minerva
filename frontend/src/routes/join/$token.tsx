import { createFileRoute, useNavigate } from "@tanstack/react-router"
import { useEffect, useState } from "react"
import { api } from "@/lib/api"
import { Skeleton } from "@/components/ui/skeleton"

export const Route = createFileRoute("/join/$token")({
  component: JoinPage,
})

function JoinPage() {
  const { token } = Route.useParams()
  const navigate = useNavigate()
  const [error, setError] = useState<string | null>(null)

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
        setError(e instanceof Error ? e.message : "Failed to join course")
      })
  }, [token, navigate])

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center py-20 gap-4">
        <p className="text-destructive text-lg">{error}</p>
      </div>
    )
  }

  return (
    <div className="flex flex-col items-center justify-center py-20 gap-4">
      <Skeleton className="h-6 w-48" />
      <p className="text-muted-foreground">Joining course...</p>
    </div>
  )
}

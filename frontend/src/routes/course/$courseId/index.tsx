import { createFileRoute, useNavigate } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { courseQuery, conversationsQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import type { Conversation } from "@/lib/types"

export const Route = createFileRoute("/course/$courseId/")({
  component: CourseIndex,
})

function CourseIndex() {
  const { courseId } = Route.useParams()
  const navigate = useNavigate()
  const { data: course } = useQuery(courseQuery(courseId))
  const { data: conversations, isLoading } = useQuery(conversationsQuery(courseId))
  const queryClient = useQueryClient()

  const createConversation = useMutation({
    mutationFn: () =>
      api.post<Conversation>(`/courses/${courseId}/conversations`, {}),
    onSuccess: (conv) => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "conversations"],
      })
      navigate({
        to: "/course/$courseId/$conversationId",
        params: { courseId, conversationId: conv.id },
      })
    },
  })

  // If conversations exist, redirect to the most recent one
  if (conversations && conversations.length > 0) {
    navigate({
      to: "/course/$courseId/$conversationId",
      params: { courseId, conversationId: conversations[0].id },
      replace: true,
    })
  }

  return (
    <div className="flex flex-col items-center justify-center h-[calc(100vh-200px)] gap-4">
      {isLoading ? (
        <Skeleton className="h-10 w-40" />
      ) : (
        <>
          <h2 className="text-xl font-semibold">{course?.name}</h2>
          <p className="text-muted-foreground">No conversations yet.</p>
          <Button
            onClick={() => createConversation.mutate()}
            disabled={createConversation.isPending}
          >
            Start Chatting
          </Button>
        </>
      )}
    </div>
  )
}

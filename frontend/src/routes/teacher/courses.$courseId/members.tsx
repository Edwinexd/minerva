import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { courseMembersQuery } from "@/lib/queries"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { useState } from "react"

export const Route = createFileRoute("/teacher/courses/$courseId/members")({
  component: MembersPage,
})

function MembersPage() {
  const { courseId } = Route.useParams()
  const { data: members, isLoading } = useQuery(courseMembersQuery(courseId))
  const queryClient = useQueryClient()
  const [eppn, setEppn] = useState("")
  const [role, setRole] = useState("student")

  const addMutation = useMutation({
    mutationFn: (data: { eppn: string; role: string }) =>
      api.post(`/courses/${courseId}/members`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "members"],
      })
      setEppn("")
    },
  })

  const removeMutation = useMutation({
    mutationFn: (userId: string) =>
      api.delete(`/courses/${courseId}/members/${userId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "members"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Members</CardTitle>
        <CardDescription>Manage who has access to this course</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <form
          className="flex flex-wrap gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (eppn) addMutation.mutate({ eppn, role })
          }}
        >
          <Input
            value={eppn}
            onChange={(e) => setEppn(e.target.value)}
            placeholder="username@SU.SE"
            className="flex-1 min-w-[12rem]"
          />
          <select
            value={role}
            onChange={(e) => setRole(e.target.value)}
            className="border rounded px-2 py-1 text-sm bg-background"
          >
            <option value="student">Student</option>
            <option value="ta">TA</option>
            <option value="teacher">Teacher</option>
          </select>
          <Button type="submit" disabled={addMutation.isPending}>
            Add
          </Button>
        </form>

        {isLoading && <p className="text-muted-foreground">Loading...</p>}

        <div className="space-y-2">
          {members?.map((m) => (
            <div
              key={m.user_id}
              className="flex flex-wrap items-center justify-between gap-2 py-2 border-b last:border-0"
            >
              <div className="min-w-0 break-words">
                <span className="font-medium">
                  {m.display_name || m.eppn}
                </span>
                {m.display_name && (
                  <span className="text-muted-foreground text-sm ml-2 break-all">
                    {m.eppn}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <Badge variant="outline">{m.role}</Badge>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => removeMutation.mutate(m.user_id)}
                >
                  Remove
                </Button>
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}

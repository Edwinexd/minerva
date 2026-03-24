import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { courseQuery, courseMembersQuery } from "@/lib/queries"
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
import { Label } from "@/components/ui/label"
import { Slider } from "@/components/ui/slider"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useState } from "react"
import type { Course } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId")({
  component: CourseEditPage,
})

function CourseEditPage() {
  const { courseId } = Route.useParams()
  const { data: course, isLoading } = useQuery(courseQuery(courseId))

  if (isLoading) return <p className="text-muted-foreground">Loading...</p>
  if (!course) return <p className="text-muted-foreground">Course not found</p>

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-bold tracking-tight">{course.name}</h2>

      <Tabs defaultValue="config">
        <TabsList>
          <TabsTrigger value="config">Configuration</TabsTrigger>
          <TabsTrigger value="members">Members</TabsTrigger>
          <TabsTrigger value="documents">Documents</TabsTrigger>
        </TabsList>

        <TabsContent value="config" className="mt-4">
          <CourseConfigForm course={course} />
        </TabsContent>

        <TabsContent value="members" className="mt-4">
          <MembersPanel courseId={courseId} />
        </TabsContent>

        <TabsContent value="documents" className="mt-4">
          <p className="text-muted-foreground">
            Document upload coming in Phase 3.
          </p>
        </TabsContent>
      </Tabs>
    </div>
  )
}

function CourseConfigForm({ course }: { course: Course }) {
  const queryClient = useQueryClient()
  const [name, setName] = useState(course.name)
  const [description, setDescription] = useState(course.description || "")
  const [contextRatio, setContextRatio] = useState(course.context_ratio)
  const [temperature, setTemperature] = useState(course.temperature)
  const [model, setModel] = useState(course.model)
  const [systemPrompt, setSystemPrompt] = useState(course.system_prompt || "")
  const [maxChunks, setMaxChunks] = useState(course.max_chunks)

  const mutation = useMutation({
    mutationFn: (data: Record<string, unknown>) =>
      api.put<Course>(`/courses/${course.id}`, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["courses"] })
    },
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>Course Configuration</CardTitle>
        <CardDescription>
          Configure how RAG works for this course
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className="space-y-6"
          onSubmit={(e) => {
            e.preventDefault()
            mutation.mutate({
              name,
              description: description || null,
              context_ratio: contextRatio,
              temperature,
              model,
              system_prompt: systemPrompt || null,
              max_chunks: maxChunks,
            })
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="name">Course Name</Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="description">Description</Label>
            <Textarea
              id="description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>

          <Separator />

          <div className="space-y-2">
            <Label>
              RAG Context Ratio: {Math.round(contextRatio * 100)}%
            </Label>
            <Slider
              value={[contextRatio]}
              onValueChange={(v) => setContextRatio(Array.isArray(v) ? v[0] : v)}
              min={0.1}
              max={0.95}
              step={0.05}
            />
            <p className="text-xs text-muted-foreground">
              How much of the context window is used for RAG chunks vs
              conversation history
            </p>
          </div>

          <div className="space-y-2">
            <Label>Temperature: {temperature.toFixed(2)}</Label>
            <Slider
              value={[temperature]}
              onValueChange={(v) => setTemperature(Array.isArray(v) ? v[0] : v)}
              min={0}
              max={1}
              step={0.05}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="model">Model</Label>
            <Input
              id="model"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder="llama-3.3-70b"
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="maxChunks">Max Retrieved Chunks</Label>
            <Input
              id="maxChunks"
              type="number"
              value={maxChunks}
              onChange={(e) => setMaxChunks(parseInt(e.target.value) || 10)}
              min={1}
              max={50}
            />
          </div>

          <div className="space-y-2">
            <Label htmlFor="systemPrompt">Custom System Prompt</Label>
            <Textarea
              id="systemPrompt"
              value={systemPrompt}
              onChange={(e) => setSystemPrompt(e.target.value)}
              placeholder="Optional custom instructions for the AI assistant"
              rows={4}
            />
          </div>

          <Button type="submit" disabled={mutation.isPending}>
            {mutation.isPending ? "Saving..." : "Save Configuration"}
          </Button>
          {mutation.isSuccess && (
            <span className="text-sm text-muted-foreground ml-2">Saved!</span>
          )}
          {mutation.isError && (
            <p className="text-sm text-destructive">{mutation.error.message}</p>
          )}
        </form>
      </CardContent>
    </Card>
  )
}

function MembersPanel({ courseId }: { courseId: string }) {
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
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (eppn) addMutation.mutate({ eppn, role })
          }}
        >
          <Input
            value={eppn}
            onChange={(e) => setEppn(e.target.value)}
            placeholder="username@SU.SE"
            className="flex-1"
          />
          <select
            value={role}
            onChange={(e) => setRole(e.target.value)}
            className="border rounded px-2 text-sm"
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
              className="flex items-center justify-between py-2 border-b last:border-0"
            >
              <div>
                <span className="font-medium">
                  {m.display_name || m.eppn}
                </span>
                {m.display_name && (
                  <span className="text-muted-foreground text-sm ml-2">
                    {m.eppn}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2">
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

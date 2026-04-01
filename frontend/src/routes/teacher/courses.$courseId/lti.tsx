import { createFileRoute } from "@tanstack/react-router"
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { ltiSetupQuery, ltiRegistrationsQuery } from "@/lib/queries"
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
import { Badge } from "@/components/ui/badge"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { useState } from "react"
import type { LtiRegistration } from "@/lib/types"

export const Route = createFileRoute("/teacher/courses/$courseId/lti")({
  component: LtiPage,
})

function LtiPage() {
  const { courseId } = Route.useParams()
  const queryClient = useQueryClient()
  const { data: setup } = useQuery(ltiSetupQuery(courseId))
  const { data: registrations, isLoading } = useQuery(ltiRegistrationsQuery(courseId))
  const [showForm, setShowForm] = useState(false)
  const [name, setName] = useState("")
  const [issuer, setIssuer] = useState("")
  const [clientId, setClientId] = useState("")
  const [copiedField, setCopiedField] = useState<string | null>(null)

  const createMutation = useMutation({
    mutationFn: (data: {
      name: string
      issuer: string
      client_id: string
    }) => api.post<LtiRegistration>(`/courses/${courseId}/lti`, data),
    onSuccess: () => {
      setShowForm(false)
      setName("")
      setIssuer("")
      setClientId("")
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "lti"],
      })
    },
  })

  const deleteMutation = useMutation({
    mutationFn: (regId: string) =>
      api.delete(`/courses/${courseId}/lti/${regId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "lti"],
      })
    },
  })

  function copyToClipboard(text: string, field: string) {
    navigator.clipboard.writeText(text)
    setCopiedField(field)
    setTimeout(() => setCopiedField(null), 2000)
  }

  const config = setup?.moodle_tool_config

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader>
          <CardTitle>Moodle Configuration</CardTitle>
          <CardDescription>
            Enter these values in Moodle when adding an LTI External Tool to your course.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {config ? (
            <>
              {[
                { label: "Tool URL", value: config.tool_url, key: "tool_url" },
                { label: "LTI version", value: config.lti_version, key: "lti_version" },
                { label: "Public key type", value: config.public_key_type, key: "public_key_type" },
                { label: "Public keyset URL", value: config.public_keyset_url, key: "keyset" },
                { label: "Initiate login URL", value: config.initiate_login_url, key: "login" },
                { label: "Redirection URI(s)", value: config.redirection_uris, key: "redirect" },
                { label: "Custom parameters", value: config.custom_parameters, key: "custom" },
              ].map(({ label, value, key }) => (
                <div key={key} className="flex items-center justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <Label className="text-xs text-muted-foreground">{label}</Label>
                    <code className="block text-sm bg-muted px-2 py-1 rounded truncate">{value}</code>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="shrink-0"
                    onClick={() => copyToClipboard(value, key)}
                  >
                    {copiedField === key ? "Copied!" : "Copy"}
                  </Button>
                </div>
              ))}
              <Separator />
              <div className="text-sm text-muted-foreground space-y-1">
                <p>The <strong>custom parameter</strong> <code>user_eppn=$User.username</code> links Moodle users to their Minerva identity. Without it, students launched from Moodle will appear as separate users.</p>
                <p>Under <strong>Privacy</strong>, "Share launcher's name" is optional (populates display names).</p>
              </div>
            </>
          ) : (
            <div className="space-y-2">
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>LTI Registrations</CardTitle>
          <CardDescription>
            After configuring the tool in Moodle, copy Moodle's platform details here to complete the connection.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {!showForm && (
            <Button onClick={() => setShowForm(true)}>Add Moodle Connection</Button>
          )}

          {showForm && (
            <form
              className="space-y-3 rounded-md border p-4"
              onSubmit={(e) => {
                e.preventDefault()
                createMutation.mutate({
                  name: name.trim(),
                  issuer: issuer.trim(),
                  client_id: clientId.trim(),
                })
              }}
            >
              <p className="text-sm text-muted-foreground">
                Copy these values from Moodle's tool registration details.
              </p>
              <div className="space-y-2">
                <Label htmlFor="lti-name">Name</Label>
                <Input id="lti-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Moodle HT2025" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-issuer">Platform ID (issuer)</Label>
                <Input id="lti-issuer" value={issuer} onChange={(e) => setIssuer(e.target.value)} placeholder="https://moodle.example.com" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="lti-client-id">Client ID</Label>
                <Input id="lti-client-id" value={clientId} onChange={(e) => setClientId(e.target.value)} />
              </div>

              {createMutation.isError && (
                <p className="text-sm text-destructive">{createMutation.error.message}</p>
              )}

              <div className="flex gap-2">
                <Button type="submit" disabled={createMutation.isPending || !issuer.trim() || !clientId.trim()}>
                  {createMutation.isPending ? "Saving..." : "Save Registration"}
                </Button>
                <Button type="button" variant="outline" onClick={() => setShowForm(false)}>
                  Cancel
                </Button>
              </div>
            </form>
          )}

          {isLoading && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
            </div>
          )}

          {registrations && registrations.length === 0 && !showForm && (
            <p className="text-sm text-muted-foreground py-4 text-center">
              No LTI connections yet. Configure the tool in Moodle first using the values above, then add the connection here.
            </p>
          )}

          <div className="space-y-3">
            {registrations?.map((reg) => (
              <div
                key={reg.id}
                className="flex items-center justify-between py-2 border-b last:border-0"
              >
                <div className="space-y-1 flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-sm">{reg.name}</span>
                    <Badge variant="secondary">{reg.client_id}</Badge>
                  </div>
                  <div className="text-xs text-muted-foreground truncate">{reg.issuer}</div>
                </div>
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => deleteMutation.mutate(reg.id)}
                  disabled={deleteMutation.isPending}
                >
                  Remove
                </Button>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}

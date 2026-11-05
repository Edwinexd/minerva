import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useNavigate } from "@tanstack/react-router"
import { useId, useRef } from "react"
import { useTranslation } from "react-i18next"
import { Paperclip, X } from "lucide-react"
import { api } from "@/lib/api"
import { chatLink } from "@/lib/workspace-routes"
import { attachmentsQuery } from "@/lib/queries"
import type { ConversationAttachment } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { ErrorText } from "@/components/ui/error-text"

/** File types the backend extracts text from (`routes::attachments`). */
const ACCEPT = ".pdf,.txt,.md,.html,.htm,application/pdf,text/plain,text/html"

/**
 * Files of a credit transfer case. With no conversation yet, attaching the
 * first file starts the case and opens it; inside a case it lists the
 * files, attaches more and removes them. Only the case's owner ever sees
 * this (the backend keeps cases private to their author).
 */
export function AttachmentsBar({
  courseId,
  conversationId,
}: {
  courseId: string
  conversationId: string | null
}) {
  const { t } = useTranslation("student")
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const inputRef = useRef<HTMLInputElement>(null)
  const hintId = useId()

  const { data: attachments } = useQuery({
    ...attachmentsQuery(courseId, conversationId ?? ""),
    enabled: conversationId !== null,
  })

  const upload = useMutation({
    mutationFn: (file: File) =>
      conversationId === null
        ? api.upload<{ conversation_id: string; attachment: ConversationAttachment }>(
            `/workspaces/${courseId}/conversations/attachments`,
            file,
          )
        : api
            .upload<ConversationAttachment>(
              `/workspaces/${courseId}/conversations/${conversationId}/attachments`,
              file,
            )
            .then((attachment) => ({ conversation_id: conversationId, attachment })),
    onSuccess: ({ conversation_id }) => {
      queryClient.invalidateQueries({ queryKey: ["courses", courseId, "conversations"] })
      queryClient.invalidateQueries({ queryKey: ["courses", courseId, "credit-transfer"] })
      if (conversationId === null) {
        navigate(chatLink(courseId, conversation_id))
      }
    },
  })

  const remove = useMutation({
    mutationFn: (id: string) =>
      api.delete(`/workspaces/${courseId}/conversations/${conversationId}/attachments/${id}`),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: attachmentsQuery(courseId, conversationId ?? "").queryKey,
      }),
  })

  const picker = (
    <>
      <input
        ref={inputRef}
        type="file"
        accept={ACCEPT}
        className="sr-only"
        tabIndex={-1}
        aria-hidden="true"
        onChange={(e) => {
          const file = e.target.files?.[0]
          e.target.value = ""
          if (file) upload.mutate(file)
        }}
      />
      <Button
        type="button"
        variant={conversationId === null ? "default" : "outline"}
        size="sm"
        disabled={upload.isPending}
        aria-describedby={hintId}
        onClick={() => inputRef.current?.click()}
      >
        <Paperclip className="w-4 h-4 mr-1" aria-hidden="true" />
        {upload.isPending
          ? t("attachments.uploading")
          : conversationId === null
            ? t("attachments.attachFirst")
            : t("attachments.attachAnother")}
      </Button>
      <span id={hintId} className="text-xs text-muted-foreground">
        {t("attachments.formats")}
      </span>
    </>
  )

  if (conversationId === null) {
    return (
      <section className="border rounded-md p-4 mb-3 space-y-2">
        <h2 className="font-semibold">{t("attachments.startTitle")}</h2>
        <p className="text-sm text-muted-foreground">{t("attachments.startHint")}</p>
        <div className="flex flex-wrap items-center gap-2">{picker}</div>
        {upload.isError && <ErrorText error={upload.error} />}
      </section>
    )
  }

  return (
    <section
      className="border-b pb-2 mb-2 space-y-2 print:hidden"
      aria-label={t("attachments.listLabel")}
    >
      {attachments && attachments.length > 0 && (
        <ul className="flex flex-wrap gap-2">
          {attachments.map((a) => (
            <li
              key={a.id}
              className="flex items-center gap-1 rounded-md border bg-muted px-2 py-1 text-sm"
            >
              <Paperclip className="w-3.5 h-3.5" aria-hidden="true" />
              <span className="max-w-64 truncate" title={a.filename}>
                {a.filename}
              </span>
              {a.text_chars < 200 && (
                <span className="text-xs text-destructive">({t("attachments.noText")})</span>
              )}
              <button
                type="button"
                className="ml-1 rounded p-0.5 hover:bg-background"
                aria-label={t("attachments.remove", { name: a.filename })}
                disabled={remove.isPending}
                onClick={() => remove.mutate(a.id)}
              >
                <X className="w-3.5 h-3.5" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-wrap items-center gap-2">{picker}</div>
      {(upload.isError || remove.isError) && (
        <ErrorText error={upload.error ?? remove.error} />
      )}
    </section>
  )
}

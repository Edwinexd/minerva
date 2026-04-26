import Markdown from "react-markdown"
import remarkGfm from "remark-gfm"

import { Badge } from "@/components/ui/badge"
import type { TeacherNote } from "@/lib/types"

/**
 * Inline teacher-note bubble. Shared between the Shibboleth chat page
 * (student namespace) and the embed iframe (auth namespace). The label
 * is passed in by the caller so each surface can keep its own
 * translation key without coupling this component to a namespace.
 */
export function TeacherNoteInline({
  note,
  label,
}: {
  note: TeacherNote
  label: string
}) {
  return (
    <div className="flex justify-center">
      <div className="bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-800 rounded-lg px-4 py-2 max-w-[80%]">
        <div className="flex items-center gap-2 mb-1">
          <Badge
            variant="outline"
            className="text-xs border-amber-300 dark:border-amber-700 text-amber-700 dark:text-amber-300"
          >
            {label}
          </Badge>
          {note.author_display_name && (
            <span className="text-xs text-muted-foreground">{note.author_display_name}</span>
          )}
        </div>
        <div className="prose prose-sm dark:prose-invert max-w-none">
          <Markdown remarkPlugins={[remarkGfm]}>{note.content}</Markdown>
        </div>
      </div>
    </div>
  )
}

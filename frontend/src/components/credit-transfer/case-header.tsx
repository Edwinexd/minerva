import { useTranslation } from "react-i18next"
import { Printer } from "lucide-react"
import type { Conversation } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { deletionDate, useDateFormat } from "./dates"

/**
 * Top of a credit transfer case: its title, when it was created, when it
 * will be deleted with its files, and printing it to PDF for the case
 * file in the student system.
 */
export function CaseHeader({
  conversation,
  retentionDays,
  isNew,
}: {
  conversation: Conversation | null
  retentionDays: number | null
  isNew: boolean
}) {
  const { t } = useTranslation("student")
  const formatDate = useDateFormat()
  const title = isNew
    ? t("creditTransfer.newCase")
    : (conversation?.title ?? t("creditTransfer.untitledCase"))
  return (
    <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
      <div className="min-w-0">
        <h2 className="text-xl font-semibold">{title}</h2>
        {conversation && retentionDays != null && (
          <p className="text-xs text-muted-foreground">
            {t("creditTransfer.caseDates", {
              created: formatDate(conversation.created_at),
              deleted: formatDate(deletionDate(conversation.updated_at, retentionDays)),
            })}
          </p>
        )}
        {isNew && retentionDays != null && (
          <p className="text-xs text-muted-foreground">
            {t("creditTransfer.retentionNote", { days: retentionDays })}
          </p>
        )}
      </div>
      {conversation && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="print:hidden"
          onClick={() => window.print()}
        >
          <Printer className="mr-1 h-4 w-4" aria-hidden="true" />
          {t("creditTransfer.exportPdf")}
        </Button>
      )}
    </div>
  )
}

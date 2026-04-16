import { createFileRoute } from "@tanstack/react-router"
import { useTranslation } from "react-i18next"
import { DataHandlingContent } from "@/components/data-handling"

export const Route = createFileRoute("/data-handling")({
  component: DataHandlingPage,
})

function DataHandlingPage() {
  const { t } = useTranslation("common")
  return (
    <div className="max-w-3xl mx-auto">
      <h1 className="text-2xl font-bold tracking-tight mb-6">
        {t("dataHandling.title")}
      </h1>
      <DataHandlingContent />
    </div>
  )
}

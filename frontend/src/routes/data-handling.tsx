import { createFileRoute } from "@tanstack/react-router"
import { DataHandlingContent } from "@/components/data-handling"

export const Route = createFileRoute("/data-handling")({
  component: DataHandlingPage,
})

function DataHandlingPage() {
  return (
    <div className="max-w-3xl mx-auto">
      <h1 className="text-2xl font-bold tracking-tight mb-6">
        How Minerva handles your data
      </h1>
      <DataHandlingContent />
    </div>
  )
}

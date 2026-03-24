import { createFileRoute } from "@tanstack/react-router"

export const Route = createFileRoute("/")({
  component: Home,
})

function Home() {
  return (
    <div className="flex flex-col items-center justify-center py-20">
      <h2 className="text-4xl font-bold tracking-tight mb-4">Minerva</h2>
      <p className="text-muted-foreground text-lg">
        High-performance RAG for course materials
      </p>
    </div>
  )
}

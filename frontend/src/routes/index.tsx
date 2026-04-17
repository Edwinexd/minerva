import { createFileRoute } from "@tanstack/react-router"
import { Home } from "@/components/home/home-page"

export const Route = createFileRoute("/")({
  component: Home,
})

import { beforeEach, describe, expect, it, vi } from "vitest"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { api } from "@/lib/api"
import { ChatModelsCard } from "@/components/admin/chat-models-card"
import type { AdminChatModel } from "@/lib/queries"
import { axe, renderWithProviders } from "./a11y"

vi.mock("@/lib/api", () => ({ api: { get: vi.fn(), put: vi.fn(), post: vi.fn() } }))

const model = (overrides: Partial<AdminChatModel>): AdminChatModel => ({
  model: "gpt-oss-120b", provider: "cerebras", display_name: "GPT OSS 120B",
  enabled: true, is_default: true, is_utility_default: true,
  is_figure_description_default: false, figure_description_provider_available: false,
  input_usd_per_mtok: 0.1, output_usd_per_mtok: 0.5,
  supports_logprobs: true, supports_tool_use: true,
  provider_available: true, courses_using: 0, price_updated_at: null,
  ...overrides,
})

function renderCatalog(selected = false) {
  const models = [model({}), model({ model: "gpt-6-luna", provider: "openai",
    display_name: "GPT-6 Luna", is_default: false, is_utility_default: false,
    is_figure_description_default: selected, figure_description_provider_available: true })]
  vi.mocked(api.get).mockResolvedValue({ models })
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return renderWithProviders(<QueryClientProvider client={client}><ChatModelsCard /></QueryClientProvider>)
}

beforeEach(() => { vi.clearAllMocks(); vi.mocked(api.put).mockResolvedValue({}) })

describe("figure description model catalog role", () => {
  it("lists OpenAI models from the full catalog and selects them using a radio dot", async () => {
    const user = userEvent.setup()
    const { container } = renderCatalog()
    const vision = await screen.findByRole("radio", { name: "Use GPT-6 Luna for figure descriptions" })
    expect(vision).toBeEnabled()
    expect(screen.getByRole("radio", { name: "Use GPT OSS 120B for figure descriptions" })).toBeDisabled()
    expect(screen.queryByRole("combobox")).toBeNull()
    await user.click(vision)
    await waitFor(() => expect(api.put).toHaveBeenCalledWith("/admin/chat-models/figure-description-default", { model: "gpt-6-luna" }))
    expect(api.get).toHaveBeenCalledWith("/admin/chat-models")
    expect(api.get).not.toHaveBeenCalledWith("/models")
    expect(await axe(container)).toHaveNoViolations()
  })

  it("shows the persisted dot and can disable descriptions without changing other defaults", async () => {
    const user = userEvent.setup()
    renderCatalog(true)
    expect(await screen.findByRole("radio", { name: "Use GPT-6 Luna for figure descriptions" })).toBeChecked()
    await user.click(screen.getByRole("button", { name: "Disable figure descriptions" }))
    await waitFor(() => expect(api.put).toHaveBeenCalledWith("/admin/chat-models/figure-description-default", { model: null }))
    expect(api.put).toHaveBeenCalledTimes(1)
  })
})

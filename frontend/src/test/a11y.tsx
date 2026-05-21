import type { ReactElement } from "react"
import { I18nextProvider } from "react-i18next"
import { render } from "@testing-library/react"
import { configureAxe } from "vitest-axe"
import i18n from "i18next"

import "@/i18n"

// axe-core configured to evaluate the success criteria that make up the
// standard DSV-IT requires for new sites: WCAG 2.2 level AA (which is a
// superset of 2.0 and 2.1 A/AA). Limiting `runOnly` to these tags keeps the
// suite focused on legally-relevant rules and out of best-practice noise.
export const axe = configureAxe({
  runOnly: {
    type: "tag",
    values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"],
  },
})

// Render a component with the providers real pages rely on. i18n is the only
// global context the leaf components under test touch; route/query providers
// are added per-test where a component needs them.
export function renderWithProviders(ui: ReactElement) {
  return render(<I18nextProvider i18n={i18n}>{ui}</I18nextProvider>)
}

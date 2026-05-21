// Vitest global setup shared by every test file (wired in via
// vitest.config.ts `setupFiles`).
import "@testing-library/jest-dom/vitest"
import * as axeMatchers from "vitest-axe/dist/matchers.js"
import { afterEach, expect } from "vitest"
import { cleanup } from "@testing-library/react"

// Register the `toHaveNoViolations` matcher from vitest-axe. The package ships
// no `exports` map, so the matcher entry is imported by its concrete dist path.
expect.extend(axeMatchers as Parameters<typeof expect.extend>[0])

// Unmount React trees between tests so the jsdom document starts clean and axe
// only ever sees the component currently under test.
afterEach(() => {
  cleanup()
})

declare module "vitest" {
  interface Assertion {
    toHaveNoViolations(): void
  }
  interface AsymmetricMatchersContaining {
    toHaveNoViolations(): void
  }
}

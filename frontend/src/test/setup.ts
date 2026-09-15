// Vitest global setup shared by every test file (wired in via
// vitest.config.ts `setupFiles`).
import * as jestDomMatchers from "@testing-library/jest-dom/matchers"
import type { TestingLibraryMatchers } from "@testing-library/jest-dom/matchers"
import * as axeMatchers from "vitest-axe/dist/matchers.js"
import { afterEach, expect } from "vitest"
import { cleanup } from "@testing-library/react"

// Register jest-dom's matchers directly: its `/vitest` entry does only this
// plus augment vitest 4's one-parameter `Assertion<T>`, which vitest 5's
// `Assertion<R, T>` rejects. The augmentation at the bottom types them.
expect.extend(jestDomMatchers)

// Register the `toHaveNoViolations` matcher from vitest-axe. The package ships
// no `exports` map, so the matcher entry is imported by its concrete dist path.
expect.extend(axeMatchers as Parameters<typeof expect.extend>[0])

// jsdom implements no layout, so it ships no `scrollIntoView`. The chat
// transcript calls it on mount to pin the view to the newest message, which
// would otherwise throw before axe ever sees the rendered transcript.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {}
}

// Unmount React trees between tests so the jsdom document starts clean and axe
// only ever sees the component currently under test.
afterEach(() => {
  cleanup()
})

declare module "vitest" {
  interface Assertion<R> extends TestingLibraryMatchers<unknown, R> {
    toHaveNoViolations(): R
  }
  interface AsymmetricMatchersContaining extends TestingLibraryMatchers<unknown, void> {
    toHaveNoViolations(): void
  }
}

import { defineConfig, mergeConfig } from "vitest/config"

import viteConfig from "./vite.config"

// Reuse the app's vite config (the `@` alias + react/tailwind plugins) so test
// modules resolve imports exactly like the real build, then layer the test
// environment on top. Accessibility tests render components into jsdom and run
// axe-core against the result; see src/test/a11y.tsx.
export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: "jsdom",
      globals: true,
      setupFiles: ["./src/test/setup.ts"],
      include: ["src/**/*.test.{ts,tsx}"],
      css: false,
    },
  })
)

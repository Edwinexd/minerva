import path from "path"
import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"
import { TanStackRouterVite } from "@tanstack/router-plugin/vite"

export default defineConfig({
  plugins: [TanStackRouterVite({ quoteStyle: "double" }), react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    proxy: {
      "/api": process.env.VITE_API_URL || "http://localhost:3000",
      // Only the LMS-facing LTI endpoints live on the backend. /lti/bind is an
      // SPA route; a bare "/lti" prefix would send it to the backend, which
      // would proxy it right back to Vite -> loop -> 502.
      "^/lti/(login|launch|jwks|icon\\.(svg|png))$":
        process.env.VITE_API_URL || "http://localhost:3000",
    },
  },
})

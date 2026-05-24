import { createFileRoute } from "@tanstack/react-router"
import { LtiSetupScopePage } from "@/components/lti-setup-scope-page"

// LTI 1.3 Dynamic Registration "trust scope" step. The server-side dynreg
// handler at /lti/dynamic-register completes the spec handshake and 303s
// here so the human in the LMS popup can pick which eppn domains to
// suggest before a Minerva integrator approves the platform. Public:
// loads without a Minerva session (RootLayout skips its `userQuery` on
// `/lti/setup` paths, same pattern as `/lti/bind`).
export const Route = createFileRoute("/lti/setup/$platformId")({
  component: LtiSetupScopePage,
  validateSearch: (
    s: Record<string, unknown>,
  ): { name?: string; issuer?: string } => ({
    name: typeof s.name === "string" ? s.name : undefined,
    issuer: typeof s.issuer === "string" ? s.issuer : undefined,
  }),
})

import { createFileRoute } from "@tanstack/react-router"
import { LtiApprovePlatformPage } from "@/components/admin/lti-approve-platform-page"

// Focused approve page deep-linked from the dynreg iframe's "Open Minerva
// to approve" button. Sibling of /admin/lti (the full platforms list);
// integrators land here, see only the one pending row's details + scope
// form, and confirm. Avoids hunting through the platforms list for the
// matching row.
export const Route = createFileRoute("/admin/lti-approve/$platformId")({
  component: LtiApprovePlatformPage,
})

import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Label } from "@/components/ui/label"
import { useCopyFeedback } from "@/lib/use-copy-feedback"
import type { LtiSetup } from "@/lib/types"

/// The eight values an LMS admin pastes when registering the tool by
/// hand. Rendered identically on the teacher course tab and the
/// site-wide admin tab; only the i18n namespace + key prefix differ, so
/// those come in as props. Each row owns its own copy button, and the
/// hook's key is the row key so only the copied row flips its label.
export function LtiManualConfigTable({
  config,
  ns,
  keyPrefix,
}: {
  config: LtiSetup["moodle_tool_config"]
  ns: "teacher" | "admin"
  keyPrefix: string
}) {
  const { t } = useTranslation(ns, { keyPrefix })
  const { t: tCommon } = useTranslation("common")
  const { copiedKey, copy } = useCopyFeedback()

  const rows = [
    { label: t("toolUrl"), value: config.tool_url, key: "tool_url" },
    { label: t("ltiVersion"), value: config.lti_version, key: "lti_version" },
    { label: t("publicKeyType"), value: config.public_key_type, key: "public_key_type" },
    { label: t("publicKeysetUrl"), value: config.public_keyset_url, key: "keyset" },
    { label: t("initiateLoginUrl"), value: config.initiate_login_url, key: "login" },
    { label: t("redirectionUris"), value: config.redirection_uris, key: "redirect" },
    { label: t("customParameters"), value: config.custom_parameters, key: "custom" },
    { label: t("iconUrl"), value: config.icon_url, key: "icon" },
  ]

  return (
    <>
      {rows.map(({ label, value, key }) => (
        <div key={key} className="flex items-center justify-between gap-4">
          <div className="min-w-0 flex-1">
            <Label className="text-xs text-muted-foreground">{label}</Label>
            <code className="block text-sm bg-muted px-2 py-1 rounded truncate">{value}</code>
          </div>
          <Button
            variant="outline"
            size="sm"
            className="shrink-0"
            onClick={() => void copy(value, key)}
          >
            {copiedKey === key ? t("copied") : tCommon("actions.copy")}
          </Button>
          <output className="sr-only">
            {copiedKey === key ? t("copied") : ""}
          </output>
        </div>
      ))}
    </>
  )
}

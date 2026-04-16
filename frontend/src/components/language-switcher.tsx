import { useTranslation } from "react-i18next"
import { SUPPORTED_LANGUAGES, type SupportedLanguage } from "@/i18n"

const LABELS: Record<SupportedLanguage, string> = {
  en: "English",
  sv: "Svenska",
}

export function LanguageSwitcher() {
  const { i18n, t } = useTranslation()
  const active = (i18n.resolvedLanguage ?? "en") as SupportedLanguage

  return (
    <label className="flex items-center gap-1.5 text-sm text-muted-foreground">
      <span className="sr-only">{t("language.label")}</span>
      <select
        value={active}
        onChange={(e) => {
          void i18n.changeLanguage(e.target.value)
        }}
        className="border rounded px-2 py-1 text-sm bg-background"
        aria-label={t("language.label")}
      >
        {SUPPORTED_LANGUAGES.map((lng) => (
          <option key={lng} value={lng}>
            {LABELS[lng]}
          </option>
        ))}
      </select>
    </label>
  )
}

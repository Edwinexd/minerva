import i18n from "i18next"
import LanguageDetector from "i18next-browser-languagedetector"
import { initReactI18next } from "react-i18next"

import enCommon from "./locales/en/common.json"
import enErrors from "./locales/en/errors.json"
import enAdmin from "./locales/en/admin.json"
import enTeacher from "./locales/en/teacher.json"
import enStudent from "./locales/en/student.json"
import enAuth from "./locales/en/auth.json"

import svCommon from "./locales/sv/common.json"
import svErrors from "./locales/sv/errors.json"
import svAdmin from "./locales/sv/admin.json"
import svTeacher from "./locales/sv/teacher.json"
import svStudent from "./locales/sv/student.json"
import svAuth from "./locales/sv/auth.json"

export const LANGUAGE_STORAGE_KEY = "minerva-language"
export const SUPPORTED_LANGUAGES = ["en", "sv"] as const
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number]

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    fallbackLng: "en",
    supportedLngs: SUPPORTED_LANGUAGES as readonly string[],
    nonExplicitSupportedLngs: true,
    ns: ["common", "errors", "admin", "teacher", "student", "auth"],
    defaultNS: "common",
    interpolation: { escapeValue: false },
    detection: {
      order: ["localStorage", "navigator", "htmlTag"],
      lookupLocalStorage: LANGUAGE_STORAGE_KEY,
      caches: ["localStorage"],
    },
    resources: {
      en: {
        common: enCommon,
        errors: enErrors,
        admin: enAdmin,
        teacher: enTeacher,
        student: enStudent,
        auth: enAuth,
      },
      sv: {
        common: svCommon,
        errors: svErrors,
        admin: svAdmin,
        teacher: svTeacher,
        student: svStudent,
        auth: svAuth,
      },
    },
  })

i18n.on("languageChanged", (lng) => {
  document.documentElement.lang = lng
})

export default i18n

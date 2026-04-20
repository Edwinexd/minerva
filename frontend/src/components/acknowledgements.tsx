import { useTranslation } from "react-i18next"

interface Credit {
  name: string
  email?: string
  role: string
}

/**
 * People (outside the core dev team) who contributed work that ships with
 * Minerva. Add new entries at the top of the relevant section.
 */
const CREDITS: Credit[] = [
  {
    name: "Tilly Makrof-Johansson",
    email: "tilly.makrof-johansson@dsv.su.se",
    role: "acknowledgements.roles.logoDesign",
  },
]

export function AcknowledgementsPage() {
  const { t } = useTranslation("common")
  return (
    <div className="max-w-3xl mx-auto">
      <h1 className="text-2xl font-bold tracking-tight mb-6">
        {t("acknowledgements.title")}
      </h1>

      <p className="text-sm text-muted-foreground mb-8 leading-relaxed">
        {t("acknowledgements.intro")}
      </p>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("acknowledgements.contributorsHeading")}
        </h2>
        <ul className="divide-y border rounded-md">
          {CREDITS.map((c) => (
            <li key={c.name} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 px-4 py-3 text-sm">
              <div className="min-w-0">
                <span className="font-medium">{c.name}</span>
                {c.email && (
                  <>
                    {" "}
                    <a
                      href={`mailto:${c.email}`}
                      className="text-muted-foreground underline hover:text-foreground break-all"
                    >
                      {c.email}
                    </a>
                  </>
                )}
              </div>
              <span className="text-muted-foreground">{t(c.role)}</span>
            </li>
          ))}
        </ul>
      </section>
    </div>
  )
}

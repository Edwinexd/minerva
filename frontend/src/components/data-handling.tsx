/**
 * Shared disclosure copy rendered both on the standalone `/data-handling`
 * page and inside the student first-use modal. Factored into a component so
 * the two stay in sync.
 */
export function DataHandlingContent() {
  return (
    <div className="space-y-5 text-sm leading-relaxed">
      <section className="space-y-2">
        <h2 className="font-semibold text-base">What we store</h2>
        <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
          <li>
            Your identity from Shibboleth or LTI: your SU username (eppn) and
            display name. External-invite users see pseudonymized names of
            others in the course.
          </li>
          <li>
            Your conversations, messages, and any feedback you submit on AI
            answers.
          </li>
          <li>Course materials uploaded by teachers or synced from Canvas / Moodle.</li>
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">Course staff can read your conversations</h2>
        <p className="text-muted-foreground">
          Teachers and TAs in your course can see every conversation you have
          there. Other students can&apos;t, unless a teacher explicitly shares
          one with the class.
        </p>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">Where your messages go</h2>
        <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
          <li>
            Chat messages and relevant course-material excerpts are sent to{" "}
            <strong>Cerebras</strong> for AI inference.
          </li>
          <li>
            If your course&apos;s teacher has enabled OpenAI embeddings, your
            questions are additionally sent to <strong>OpenAI</strong> to compute
            a search vector. Otherwise embeddings run locally on the Minerva
            server.
          </li>
          <li>Nothing else leaves the Minerva server.</li>
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">How long we keep it</h2>
        <p className="text-muted-foreground">
          Conversations and messages are retained for as long as the course
          exists.
        </p>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          Canvas / Moodle / Play integrations (teachers)
        </h2>
        <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
          <li>
            <strong>Canvas:</strong> Minerva pulls course files and page
            content only (no submissions, rosters, or grades). Your Canvas
            API token is stored in the Minerva database in plaintext; revoke
            it in Minerva or Canvas to disconnect.
          </li>
          <li>
            <strong>Moodle plugin:</strong> pushes course materials (files,
            page/book/label HTML, URLs) and a list of enrolled students (eppn
            + display name) to Minerva.
          </li>
          <li>
            <strong>LTI launches:</strong> when Moodle or Canvas launches
            Minerva in an iframe, the platform passes the student&apos;s
            identifier and (if configured) display name to Minerva. A user row
            is created on first launch.
          </li>
          <li>
            <strong>Play (play.dsv.su.se):</strong> when a `.url` document
            points at Play, the VTT transcript is fetched and stored as
            searchable text inside the course.
          </li>
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">Contact</h2>
        <p className="text-muted-foreground">
          Questions: <a href="mailto:lambda@dsv.su.se" className="underline hover:text-foreground">lambda@dsv.su.se</a>
        </p>
      </section>
    </div>
  )
}

# Screenshots

These are the static images linked from the top-level [README](../../README.md).

To regenerate them after a UI change:

1. Start the dev stack:

   ```bash
   cp .env.example .env  # CEREBRAS_/OPENAI_ keys may be stubs for screenshots
   docker compose -f docker-compose.yml up -d
   ```

2. Seed the fixture cast. This is what fills the shots: courses, members,
   documents and five weeks of AI spend for the teacher usage page.

   ```bash
   MINERVA_ADMINS=<your-username> scripts/seed-dev.sh
   ```

3. Run the Playwright capture script (uses the dev `X-Dev-User` header to skip
   Shibboleth; it sends `<your-username>@su.se`, the identity the seeder and the
   dev-auth fallback both use, so edit that constant if your admin differs):

   ```bash
   mkdir -p /tmp/minerva-shots && cd /tmp/minerva-shots
   npm init -y && npm i playwright
   npx playwright install chromium --with-deps
   node $OLDPWD/docs/screenshots/regenerate.mjs
   ```

The script writes its outputs back into this directory, overwriting the existing PNGs. Commit only the regenerated PNGs that actually changed.

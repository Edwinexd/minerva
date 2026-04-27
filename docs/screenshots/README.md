# Screenshots

These are the static images linked from the top-level [README](../../README.md).

To regenerate them after a UI change:

1. Start the dev stack:

   ```bash
   cp .env.example .env  # CEREBRAS_/OPENAI_ keys may be stubs for screenshots
   docker compose -f docker-compose.yml up -d
   ```

2. Optionally seed a few demo courses (any teacher account works):

   ```bash
   curl -s -X POST http://localhost:3000/api/courses \
     -H 'X-Dev-User: edsu8469' -H 'Content-Type: application/json' \
     -d '{"name":"Discrete Mathematics","description":"Sets, relations, and graph theory."}'
   curl -s -X POST http://localhost:3000/api/courses \
     -H 'X-Dev-User: edsu8469' -H 'Content-Type: application/json' \
     -d '{"name":"Information Retrieval 2026","description":"Vector search, ranking, and evaluation.","strategy":"flare"}'
   ```

3. Run the Playwright capture script (uses the dev `X-Dev-User` header to skip Shibboleth):

   ```bash
   mkdir -p /tmp/minerva-shots && cd /tmp/minerva-shots
   npm init -y && npm i playwright
   npx playwright install chromium --with-deps
   node $OLDPWD/docs/screenshots/regenerate.mjs
   ```

The script writes its outputs back into this directory, overwriting the existing PNGs. Commit only the regenerated PNGs that actually changed.

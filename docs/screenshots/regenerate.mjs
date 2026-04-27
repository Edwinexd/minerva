// Regenerate the README screenshots from a fresh dev stack.
//
// Usage:
//   1. Start the dev stack:
//        cp .env.example .env  # edit if needed; CEREBRAS_/OPENAI_ keys can be stubs
//        docker compose -f docker-compose.yml up -d
//   2. Optionally seed a couple of demo courses (any teacher account works):
//        curl -s -X POST http://localhost:3000/api/courses \
//          -H 'X-Dev-User: edsu8469' -H 'Content-Type: application/json' \
//          -d '{"name":"Discrete Mathematics","description":"Sets, relations, and graph theory."}'
//        curl -s -X POST http://localhost:3000/api/courses \
//          -H 'X-Dev-User: edsu8469' -H 'Content-Type: application/json' \
//          -d '{"name":"Information Retrieval 2026","description":"Vector search, ranking, and evaluation.","strategy":"flare"}'
//   3. Install playwright in a temp dir and run this script:
//        mkdir -p /tmp/minerva-shots && cd /tmp/minerva-shots
//        npm init -y && npm i playwright
//        npx playwright install chromium --with-deps
//        node /home/edwin/repos/Edwinexd/minerva/docs/screenshots/regenerate.mjs

import { chromium } from "playwright";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname } from "node:path";

const BASE = "http://localhost:3000";
const OUT = dirname(fileURLToPath(import.meta.url));
await mkdir(OUT, { recursive: true });

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 2,
  extraHTTPHeaders: { "X-Dev-User": "edsu8469" },
});
const page = await ctx.newPage();

async function snap(path, file, opts = {}) {
  console.log(`> ${path} -> ${file}`);
  await page.goto(`${BASE}${path}`, { waitUntil: "domcontentloaded", timeout: 30000 });
  try {
    await page.waitForSelector("h1, h2, main, [role=main]", { timeout: 5000 });
  } catch {}
  await page.waitForTimeout(opts.settle ?? 1500);
  await page.screenshot({ path: `${OUT}/${file}`, fullPage: opts.fullPage ?? false });
}

await snap("/", "01-home-courses.png", { fullPage: true });

const firstCourseId = await page.evaluate(async () => {
  const r = await fetch("/api/courses");
  const j = await r.json();
  return j[0]?.id;
});

if (firstCourseId) {
  await snap(`/course/${firstCourseId}/new`, "02-chat-new.png", { settle: 2000 });
  await snap(`/teacher/courses/${firstCourseId}`, "03-teacher-course-config.png", {
    fullPage: true,
    settle: 2500,
  });
}

await snap("/admin/system", "04-admin-system-embedding.png", { fullPage: true, settle: 2500 });
await snap("/admin/courses", "05-admin-courses.png", { fullPage: true, settle: 2000 });
await snap("/admin/users", "06-admin-users.png", { fullPage: true, settle: 2000 });
await snap("/admin/rules", "07-admin-rules.png", { fullPage: true, settle: 2000 });
await snap("/acknowledgements", "08-acknowledgements.png", { fullPage: true, settle: 1500 });

await browser.close();
console.log("done");

// Renders the committed SVG icon sources to PNG, reproducibly (Chromium through the
// Playwright of the dev container, the same way scripts/ui-check.mjs finds it).
//
//   node scripts/render-icons.mjs tray   the two tray icons: src-tauri/icons-src/tray-*.svg
//                                         to src-tauri/icons/tray/*.png (32x32, transparent)
//   node scripts/render-icons.mjs app    src-tauri/icons-src/icon.svg to
//                                         src-tauri/icons-src/icon-1024.png, the master that
//                                         `pnpm tauri icon src-tauri/icons-src/icon-1024.png`
//                                         turns into the full set in src-tauri/icons/
//   node scripts/render-icons.mjs        both
//
// Needs no project dependency. Point PLAYWRIGHT_DIR at a node_modules folder that has
// playwright if it is not at /opt/node-tools.

import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import fs from "node:fs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(process.env.PLAYWRIGHT_DIR ? path.join(process.env.PLAYWRIGHT_DIR, "x.js") : "/opt/node-tools/node_modules/x.js");
const { chromium } = require("playwright");

const src = path.join(repo, "src-tauri", "icons-src");
const icons = path.join(repo, "src-tauri", "icons");

const JOBS = {
  tray: [
    { svg: "tray-normal.svg", png: path.join(icons, "tray", "tray-normal.png"), size: 32 },
    { svg: "tray-paused.svg", png: path.join(icons, "tray", "tray-paused.png"), size: 32 },
  ],
  app: [{ svg: "icon.svg", png: path.join(src, "icon-1024.png"), size: 1024 }],
};

const wanted = process.argv[2] ? [process.argv[2]] : Object.keys(JOBS);
const browser = await chromium.launch();
try {
  for (const name of wanted) {
    const jobs = JOBS[name];
    if (!jobs) throw new Error(`unknown target ${name} (use tray or app)`);
    for (const j of jobs) {
      const svg = fs.readFileSync(path.join(src, j.svg), "utf8");
      const page = await browser.newPage({ viewport: { width: j.size, height: j.size }, deviceScaleFactor: 1 });
      const url = `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
      await page.setContent(
        `<style>html,body{margin:0;background:transparent}img{display:block;width:${j.size}px;height:${j.size}px}</style><img src="${url}">`,
      );
      await page.waitForFunction(() => document.images[0].complete);
      fs.mkdirSync(path.dirname(j.png), { recursive: true });
      await page.screenshot({ path: j.png, omitBackground: true, clip: { x: 0, y: 0, width: j.size, height: j.size } });
      await page.close();
      console.log(`${path.relative(repo, j.png)} (${j.size}x${j.size})`);
    }
  }
} finally {
  await browser.close();
}

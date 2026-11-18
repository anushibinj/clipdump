// Browser checks of the UI against the fake backend (src/dev, `vite --mode mock`).
//
//   node scripts/ui-check.mjs keys    keyboard flow assertions (exit code 1 on a failure)
//   node scripts/ui-check.mjs perf    keystroke to rendered timings with 1,000 fake items
//   node scripts/ui-check.mjs shots   screenshots for docs/screenshots (needs --out DIR). The
//                                     committed PNGs were palette-quantized with Pillow afterwards
//                                     (96 colors, no dithering) to stay under 1 MB in total.
//
// Options: --url http://localhost:1430   use a running `pnpm dev:mock` instead of
//                                          starting a private dev server
//          --out DIR                      where `shots` writes the PNG files
//          --prod                         build the mock app for production (minified, React
//                                          production mode) and serve that; use it for `perf`
//
// Uses the Playwright that is preinstalled in the dev container (it is not a
// project dependency; point PLAYWRIGHT_DIR at a node_modules folder that has it).
// Run it from a scratch directory, not from the repo root:
//
//   cd "$SCRATCH" && node /home/user/clipdump/scripts/ui-check.mjs keys
//
// The numbers from `perf` are relative: Linux Chromium and a JavaScript mock
// backend, not WebView2 and the Rust search. The Windows figure is in docs/manual-checks.md.

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import fs from "node:fs";
import os from "node:os";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(process.env.PLAYWRIGHT_DIR ? path.join(process.env.PLAYWRIGHT_DIR, "x.js") : "/opt/node-tools/node_modules/x.js");
const { chromium } = require("playwright");

const args = process.argv.slice(2);
const mode = args[0];
const opt = (name) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
};

// ---------------------------------------------------------------------------
// Dev server
// ---------------------------------------------------------------------------

async function startServer() {
  const given = opt("url");
  if (given) return { url: given, stop: async () => {} };
  const port = 1431;
  let builtDir = null;
  let serveArgs = ["exec", "vite", "--mode", "mock", "--port", String(port), "--strictPort"];
  if (args.includes("--prod")) {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "clipdump-mock-"));
    await new Promise((resolve, reject) => {
      const b = spawn("pnpm", ["exec", "vite", "build", "--mode", "mock", "--outDir", outDir, "--emptyOutDir"], { cwd: repo, stdio: "ignore" });
      b.on("exit", (c) => (c === 0 ? resolve() : reject(new Error(`mock build failed (${c})`))));
    });
    builtDir = outDir;
    serveArgs = ["exec", "vite", "preview", "--mode", "mock", "--outDir", outDir, "--port", String(port), "--strictPort"];
  }
  const child = spawn("pnpm", serveArgs, {
    cwd: repo,
    stdio: "ignore",
    detached: true,
  });
  const url = `http://localhost:${port}/`;
  for (let i = 0; i < 100; i += 1) {
    try {
      const r = await fetch(url);
      if (r.ok) break;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  return {
    url,
    stop: async () => {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch {
        // already gone
      }
      if (builtDir) fs.rmSync(builtDir, { recursive: true, force: true });
    },
  };
}

async function openPage(browser, url, { scheme = "light", dpr = 1, width = 720, height = 460 } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: dpr, colorScheme: scheme });
  const page = await ctx.newPage();
  const problems = [];
  page.on("console", (m) => {
    if (m.type() === "error" || m.type() === "warning") problems.push(`console ${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  await page.goto(url);
  await page.waitForSelector('[role="option"]');
  return { page, ctx, problems };
}

// ---------------------------------------------------------------------------
// keys
// ---------------------------------------------------------------------------

async function keys(browser, url) {
  const { page, problems } = await openPage(browser, url);
  let failed = 0;
  const check = (name, ok, detail = "") => {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `  (${detail})`}`);
    if (!ok) failed += 1;
  };
  const calls = (cmd) => page.evaluate((c) => window.__clipMock.control.calls.filter((x) => x.cmd === c), cmd);
  const lastCall = async (cmd) => (await calls(cmd)).at(-1);
  const reset = async () => {
    await page.evaluate(() => window.__clipMock.reset());
    await page.waitForTimeout(150);
    await page.locator("input[cmdk-input]").fill("");
    await page.waitForTimeout(100);
  };
  const rows = () => page.locator('[role="option"]');
  const selectedId = async () => Number(await page.locator('[role="option"][aria-selected="true"]').first().getAttribute("data-id"));
  const rowIds = () => page.locator('[role="option"]').evaluateAll((els) => els.map((e) => Number(e.getAttribute("data-id"))));
  const settle = () => page.waitForTimeout(120);

  // focus and aria
  check("input is focused on load", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));
  await page.keyboard.type("build");
  await settle();
  const first = (await rowIds())[0];
  check("typing searches and selects the first row", (await selectedId()) === first);
  check(
    "aria-activedescendant points at the selected row",
    (await page.locator("input[cmdk-input]").getAttribute("aria-activedescendant")) === `clip-row-${first}`,
  );
  check("listbox and option roles are present", (await page.locator('[role="listbox"]').count()) === 1);

  // arrows with wrap
  const ids = await rowIds();
  await page.keyboard.press("ArrowDown");
  check("ArrowDown selects the second row", (await selectedId()) === ids[1]);
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  check("ArrowUp from the first row wraps to the last", (await selectedId()) === ids.at(-1));
  await page.keyboard.press("ArrowDown");
  check("ArrowDown from the last row wraps to the first", (await selectedId()) === ids[0]);
  await page.keyboard.press("Home");
  check("Home stays with the input caret (no selection change)", (await selectedId()) === ids[0]);

  // scroll into view while arrowing through a long list
  await reset();
  for (let i = 0; i < 14; i += 1) await page.keyboard.press("ArrowDown");
  await settle();
  const visible = await page.evaluate(() => {
    const el = document.querySelector('[role="option"][aria-selected="true"]');
    const list = document.querySelector('[role="listbox"]');
    const a = el.getBoundingClientRect();
    const b = list.getBoundingClientRect();
    return a.top >= b.top - 1 && a.bottom <= b.bottom + 1;
  });
  check("the selected row is scrolled into view", visible);

  // Ctrl+Enter copies the selected id
  await reset();
  await page.keyboard.press("ArrowDown");
  const want = await selectedId();
  await page.keyboard.press("Control+Enter");
  await settle();
  const copy = await lastCall("copy_item");
  check("Ctrl+Enter copies the selected item without paste", copy?.args.id === want && copy?.args.paste === false, JSON.stringify(copy));

  // Enter copies too (auto paste is off until Phase 12)
  await reset();
  const w2 = await selectedId();
  await page.keyboard.press("Enter");
  await settle();
  const c2 = await lastCall("copy_item");
  check("Enter copies the selected item (paste only when the setting is on)", c2?.args.id === w2 && c2?.args.paste === false, JSON.stringify(c2));

  // Ctrl+1 and Ctrl+3
  await reset();
  await page.keyboard.type("a");
  await settle();
  const ids2 = await rowIds();
  await page.keyboard.press("Control+1");
  check("Ctrl+1 copies the first visible row", (await lastCall("copy_item"))?.args.id === ids2[0]);
  await page.keyboard.press("Control+3");
  check("Ctrl+3 copies the third visible row", (await lastCall("copy_item"))?.args.id === ids2[2]);
  await reset();
  await page.keyboard.type("zzzzqq");
  await settle();
  const before = (await calls("copy_item")).length;
  await page.keyboard.press("Control+5");
  check("Ctrl+5 with fewer rows does nothing", (await calls("copy_item")).length === before);

  // copy failure shows a message and the bar keeps working
  await reset();
  await page.evaluate(() => (window.__clipMock.control.copyError = "copying to the clipboard is not implemented yet (todo Phase 11)"));
  await page.keyboard.press("Control+Enter");
  await settle();
  const alert = page.locator('footer [role="alert"]');
  check("a failed copy shows an inline error", (await alert.count()) === 1 && /not implemented/.test(await alert.innerText()));
  await page.keyboard.press("ArrowDown");
  check("the bar still works after a failed copy", (await page.locator('[role="option"][aria-selected="true"]').count()) === 1);
  await page.evaluate(() => (window.__clipMock.control.copyError = null));

  // pin toggling
  await reset();
  await page.keyboard.type("meeting");
  await settle();
  const pid = await selectedId();
  const pinnedBefore = await page.locator(`#clip-row-${pid} [aria-label="Pinned"]`).count();
  await page.keyboard.press("Control+p");
  await settle();
  check("Ctrl+P calls toggle_pin with the selected id", (await lastCall("toggle_pin"))?.args.id === pid);
  const pinnedAfter = await page.locator(`#clip-row-${pid} [aria-label="Pinned"]`).count();
  check("the pin icon toggles", pinnedBefore !== pinnedAfter);
  await page.keyboard.press("Control+p");
  await settle();
  check("Ctrl+P again toggles it back", (await page.locator(`#clip-row-${pid} [aria-label="Pinned"]`).count()) === pinnedBefore);

  // delete
  await reset();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  const idsD = await rowIds();
  const did = await selectedId();
  await page.keyboard.press("Control+Delete");
  await settle();
  check("Ctrl+Delete deletes the selected item", (await lastCall("delete_item"))?.args.id === did);
  const idsAfter = await rowIds();
  check("the deleted row is gone and the neighbor takes the selection", !idsAfter.includes(did) && (await selectedId()) === idsD[3]);
  const did2 = await selectedId();
  await page.keyboard.press("Shift+Delete");
  await settle();
  check("Shift+Delete deletes too", (await lastCall("delete_item"))?.args.id === did2);
  await page.keyboard.type("x");
  await page.keyboard.press("Delete");
  check("plain Delete only edits the query", (await calls("delete_item")).length === 2);

  // clear with confirm
  await reset();
  const total = await page.evaluate(() => window.__clipMock.rows().length);
  const pinnedCount = await page.evaluate(() => window.__clipMock.rows().filter((r) => r.pinned).length);
  await page.keyboard.press("Control+Shift+Delete");
  await settle();
  const confirmText = await page.locator('footer [role="alert"]').innerText();
  check(
    "first Ctrl+Shift+Delete asks to confirm and clears nothing",
    /again to clear/.test(confirmText) && confirmText.includes(String(total - pinnedCount)) && (await calls("clear_history")).length === 0,
    confirmText,
  );
  await page.waitForTimeout(3300);
  check("the confirmation expires after 3 seconds", (await page.locator('footer [role="alert"]').count()) === 0);
  await page.keyboard.press("Control+Shift+Delete");
  await page.waitForTimeout(3300);
  await page.keyboard.press("Control+Shift+Delete");
  await settle();
  check("a press after the window expired only asks again", (await calls("clear_history")).length === 0 && (await page.locator('footer [role="alert"]').count()) === 1);
  await page.keyboard.press("Control+Shift+Delete");
  await page.waitForTimeout(400);
  check("the second press inside the window clears", (await calls("clear_history")).length === 1);
  check("pinned items survive the clear", (await rowIds()).length === pinnedCount, `${(await rowIds()).length} rows, ${pinnedCount} pinned`);
  await reset();
  await page.keyboard.press("Control+Shift+Delete");
  await page.keyboard.press("Escape");
  await settle();
  check("Esc cancels the confirmation without hiding the bar", (await page.locator('footer [role="alert"]').count()) === 0 && (await calls("hide_bar")).length === 0);

  // filters
  await reset();
  const pressed = (name) => page.locator(`[data-slot="toggle-group-item"][data-state="on"]`).innerText().then((t) => t.includes(name));
  await page.keyboard.press("Alt+2");
  await settle();
  check("Alt+2 selects Text", (await pressed("Text")) && (await lastCall("search"))?.args.filter === "text");
  check("Text filter shows only text rows", await page.locator('[role="option"]').evaluateAll((els) => els.every((e) => e.dataset.kind === "text")));
  await page.keyboard.press("Alt+3");
  await settle();
  check("Alt+3 selects Images", (await pressed("Images")) && await page.locator('[role="option"]').evaluateAll((els) => els.length > 0 && els.every((e) => e.dataset.kind === "image")));
  await page.keyboard.press("Alt+4");
  await settle();
  check("Alt+4 selects Files", (await pressed("Files")) && await page.locator('[role="option"]').evaluateAll((els) => els.length > 0 && els.every((e) => e.dataset.kind === "files")));
  await page.keyboard.press("Alt+1");
  await settle();
  check("Alt+1 selects All", await pressed("All"));
  await page.keyboard.type("img:");
  await settle();
  check("a typed img: prefix wins over the toggle", await page.locator('[role="option"]').evaluateAll((els) => els.length > 0 && els.every((e) => e.dataset.kind === "image")));

  // preview and Esc layering
  await reset();
  await page.keyboard.press("Tab");
  await settle();
  const sel = await selectedId();
  check("Tab opens the preview pane for the selected item", (await page.locator('aside[aria-label="Preview"]').count()) === 1 && (await lastCall("get_preview"))?.args.id === sel);
  check("focus stays in the input after Tab", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));
  await page.keyboard.press("ArrowDown");
  await settle();
  check("the preview follows the selection", (await lastCall("get_preview"))?.args.id === (await selectedId()));
  await page.keyboard.press("Tab");
  check("Tab again closes it", (await page.locator('aside[aria-label="Preview"]').count()) === 0);
  await page.keyboard.press("Tab");
  await page.keyboard.press("Control+,");
  await settle();
  check("Ctrl+, opens settings", (await page.locator("h2", { hasText: "Settings" }).count()) === 1);
  const hides0 = (await calls("hide_bar")).length;
  await page.keyboard.press("Escape");
  await settle();
  check("Esc closes settings first (preview stays)", (await page.locator("h2", { hasText: "Settings" }).count()) === 0 && (await page.locator('aside[aria-label="Preview"]').count()) === 1);
  await page.keyboard.press("Escape");
  await settle();
  check("Esc closes the preview next", (await page.locator('aside[aria-label="Preview"]').count()) === 0 && (await calls("hide_bar")).length === hides0);
  await page.keyboard.press("Escape");
  await settle();
  check("Esc then hides the bar", (await calls("hide_bar")).length === hides0 + 1);

  // settings
  await reset();
  await page.keyboard.press("Control+,");
  await settle();
  await page.locator("#paused").click();
  await settle();
  check("the pause switch saves through set_settings", (await lastCall("set_settings"))?.args.settings.paused === true);
  await page.locator("#max-items").fill("500");
  await page.locator("#max-items").press("Enter");
  await settle();
  check("a number field commits on Enter", (await lastCall("set_settings"))?.args.settings.maxItems === 500);
  await page.locator("#max-text").fill("0");
  await page.locator("#max-text").blur();
  await settle();
  check("an invalid number falls back to the stored value", (await page.locator("#max-text").inputValue()) === "5");
  check("disabled controls stay disabled", (await page.locator("#autostart").isDisabled()) && (await page.locator("#auto-paste").isDisabled()));
  await page.getByRole('radio', { name: 'Dark' }).click();
  check("choosing Dark theme adds the dark class", await page.evaluate(() => document.documentElement.classList.contains("dark")));
  await page.getByRole('radio', { name: 'System' }).click();
  await page.keyboard.press("Escape");
  await settle();
  check("the Paused badge shows in the search bar", (await page.getByText("Paused", { exact: true }).count()) === 1);

  // mouse: filter buttons and rows work and focus stays in the input
  await reset();
  await page.getByRole("radio", { name: /Images/ }).click();
  await settle();
  check("clicking a filter applies it and keeps focus in the input", (await lastCall("search"))?.args.filter === "images" && (await page.evaluate(() => document.activeElement?.tagName === "INPUT")));
  const second = (await rowIds())[1];
  await page.locator(`#clip-row-${second}`).click();
  await settle();
  check("clicking a row copies that item", (await lastCall("copy_item"))?.args.id === second);
  await page.getByRole("radio", { name: /All/ }).click();

  // bar:shown focuses and selects the query, bar:hidden resets the view
  await reset();
  await page.keyboard.type("abc");
  await page.locator("body").click({ position: { x: 2, y: 2 } }); // focus leaves the input
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await settle();
  const sel2 = await page.evaluate(() => {
    const i = document.querySelector("input[cmdk-input]");
    return { focused: document.activeElement === i, from: i.selectionStart, to: i.selectionEnd };
  });
  check("bar:shown focuses the input and selects its text", sel2.focused && sel2.from === 0 && sel2.to === 3, JSON.stringify(sel2));
  await page.keyboard.press("Tab");
  await page.keyboard.press("Control+,");
  await settle();
  await page.evaluate(() => window.__clipMock.emit("bar:hidden"));
  await settle();
  check(
    "bar:hidden returns to the plain search view for the next open",
    (await page.locator("h2", { hasText: "Settings" }).count()) === 0 && (await page.locator('aside[aria-label="Preview"]').count()) === 0,
  );

  // history:changed
  await reset();
  await page.evaluate(() => window.__clipMock.capture("a brand new clipboard entry"));
  await page.waitForTimeout(250);
  check("history:changed refreshes the list with the new item (after the pinned ones)", (await rows().allInnerTexts()).some((t) => t.includes("a brand new clipboard entry")));

  // stale responses
  await reset();
  await page.evaluate(() => (window.__clipMock.control.delayByQuery = { bu: 400, bui: 300 }));
  await page.keyboard.type("build", { delay: 5 });
  await page.waitForTimeout(700);
  const texts = await page.locator('[role="option"]').allInnerTexts();
  const marks = await page.locator('[role="option"] mark').allInnerTexts();
  check("slow answers to older queries are dropped", texts.length > 0 && marks.every((m) => /^build$/i.test(m.trim()) || m.length <= 5), JSON.stringify(marks.slice(0, 4)));
  check("the list shows the answer to the last query only", (await page.locator("input[cmdk-input]").inputValue()) === "build" && texts.every((t) => /build/i.test(t)), texts.slice(0, 2).join("|"));

  // highlights with non ASCII and astral characters
  await reset();
  await page.evaluate(() => window.__clipMock.capture("launch \u{1F680}\u{1F680} then celebrate \u{1F389} together"));
  await page.waitForTimeout(150);
  await page.keyboard.type("celebrate");
  await settle();
  const m1 = await page.locator('[role="option"] mark').allInnerTexts();
  check("highlights stay on the right characters after an emoji", m1.length > 0 && m1.every((m) => m.toLowerCase() === "celebrate"), JSON.stringify(m1));
  await page.locator("input[cmdk-input]").fill("brûlée");
  await settle();
  const m2 = await page.locator('[role="option"] mark').allInnerTexts();
  check("highlights work with accented characters", m2.length > 0 && m2.every((m) => m === "brûlée"), JSON.stringify(m2));

  check("no console errors or page errors", problems.length === 0, problems.slice(0, 3).join(" | "));
  console.log(failed === 0 ? "\nall checks passed" : `\n${failed} check(s) failed`);
  await page.context().close();
  return failed;
}

// ---------------------------------------------------------------------------
// perf
// ---------------------------------------------------------------------------

function pct(sorted, p) {
  return sorted[Math.min(sorted.length - 1, Math.floor((sorted.length * p) / 100))];
}

async function perf(browser, url) {
  const words = ["regression", "invoice", "revenue", "kubectl", "meeting", "paragraph", "ssh deploy", "build", "quarterly", "boarding"];
  const rowsOut = [];
  for (const [label, latency, cpu] of [
    ["mock IPC 0 ms, CPU x1", 0, 1],
    ["mock IPC 2 ms, CPU x1", 2, 1],
    ["mock IPC 2 ms, CPU x4 slowdown", 2, 4],
  ]) {
    const { page, ctx, problems } = await openPage(browser, url);
    const cdp = await ctx.newCDPSession(page);
    await page.evaluate(() => window.__clipMock.setCount(1000));
    await page.waitForTimeout(500);
    await page.evaluate((ms) => (window.__clipMock.control.latencyMs = ms), latency);
    if (cpu > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: cpu });
    const result = await page.evaluate(async (wordList) => {
      const input = document.querySelector("input");
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
      const list = document.querySelector('[role="listbox"]');
      const frame = () => new Promise((r) => requestAnimationFrame(() => setTimeout(r, 0)));
      const step = (text) =>
        new Promise((resolve) => {
          const t0 = performance.now();
          performance.mark("key");
          let done = false;
          const mo = new MutationObserver(() => {
            if (done) return;
            done = true;
            mo.disconnect();
            const commit = performance.now() - t0;
            frame().then(() => {
              performance.mark("painted");
              performance.measure("keystroke-to-next-frame", "key", "painted");
              resolve({ commit, frame: performance.now() - t0 });
            });
          });
          mo.observe(list, { childList: true, subtree: true, characterData: true, attributes: true });
          setter.call(input, text);
          input.dispatchEvent(new Event("input", { bubbles: true }));
          setTimeout(() => {
            if (!done) {
              done = true;
              mo.disconnect();
              resolve(null); // the results did not change; not a sample
            }
          }, 600);
        });
      const out = [];
      for (let round = 0; round < 3; round += 1) {
        for (const w of wordList) {
          for (let i = 1; i <= w.length; i += 1) {
            const r = await step(w.slice(0, i));
            if (r) out.push(r);
            await new Promise((res) => setTimeout(res, 8));
          }
          for (let i = w.length - 1; i >= 0; i -= 1) {
            const r = await step(w.slice(0, i));
            if (r) out.push(r);
            await new Promise((res) => setTimeout(res, 8));
          }
        }
      }
      return out;
    }, words);
    const commit = result.map((r) => r.commit).sort((a, b) => a - b);
    const frames = result.map((r) => r.frame).sort((a, b) => a - b);
    rowsOut.push({ label, n: commit.length, c50: pct(commit, 50), c95: pct(commit, 95), cmax: commit.at(-1), f50: pct(frames, 50), f95: pct(frames, 95), problems });
    await ctx.close();
  }
  {
    const { page, ctx } = await openPage(browser, url);
    await page.evaluate(() => window.__clipMock.setCount(1000));
    const t = await page.evaluate(() => {
      const times = [];
      for (const w of ["regression", "invoice", "revenue", "kubectl", "meeting", "build", "quarterly"]) {
        for (let i = 1; i <= w.length; i += 1) {
          const t0 = performance.now();
          window.__clipMock.searchNow(w.slice(0, i));
          times.push(performance.now() - t0);
        }
      }
      return times.sort((a, b) => a - b);
    });
    console.log(`the mock search alone (JavaScript, 1,000 items): p50 ${pct(t, 50).toFixed(2)} ms, p95 ${pct(t, 95).toFixed(2)} ms\n`);
    await ctx.close();
  }
  console.log("keystroke to rendered, 1,000 fake items, Chromium on Linux (relative figures, not Windows):\n");
  console.log("scenario                          samples  commit p50  commit p95  commit max  next-frame p50  next-frame p95");
  for (const r of rowsOut) {
    console.log(
      `${r.label.padEnd(33)} ${String(r.n).padStart(7)}  ${r.c50.toFixed(1).padStart(8)} ms  ${r.c95.toFixed(1).padStart(8)} ms  ${r.cmax.toFixed(1).padStart(8)} ms  ${r.f50.toFixed(1).padStart(11)} ms  ${r.f95.toFixed(1).padStart(11)} ms`,
    );
  }
  console.log("\ncommit = input event until the new rows are in the DOM (mock search + React render).");
  console.log("next-frame = until after the next animation frame; includes waiting for the 60 Hz frame boundary.");
  const bad = rowsOut.flatMap((r) => r.problems);
  if (bad.length) console.log("console problems:", bad.slice(0, 3));
  return 0;
}

// ---------------------------------------------------------------------------
// shots
// ---------------------------------------------------------------------------

async function shots(browser, url) {
  const out = opt("out");
  if (!out) throw new Error("shots needs --out DIR");
  fs.mkdirSync(out, { recursive: true });
  for (const scheme of ["light", "dark"]) {
    const { page, ctx, problems } = await openPage(browser, url, { scheme, dpr: 2 });
    // A backdrop so the translucent card and its shadow show up as they do on a desktop.
    await page.addStyleTag({ content: "html{background:#9aa3b5 !important}" });
    const input = page.locator("input[cmdk-input]");
    // Dark gets a few frames only, to keep docs/screenshots small.
    const darkSet = ["02-text-search", "06-preview-image", "07-files-missing", "12-settings"];
    const shot = async (name) => {
      if (scheme === "dark" && !darkSet.includes(name)) return;
      await page.waitForTimeout(300);
      await page.screenshot({ path: path.join(out, `${name}-${scheme}.png`) });
    };
    const all = scheme === "light";
    await shot("01-empty-query");
    await input.fill("build");
    await shot("02-text-search");
    if (all) {
      await input.fill("regression");
      await shot("03-late-match");
    }
    await input.fill("revenue");
    await shot("04-ocr-match");
    await input.fill("");
    await page.keyboard.press("Alt+3");
    await shot("05-images");
    await page.keyboard.press("Tab");
    await shot("06-preview-image");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Alt+4");
    await shot("07-files-missing");
    await page.keyboard.press("Tab");
    await shot("08-preview-files");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Alt+2");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Tab");
    await shot("09-preview-text");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Alt+1");
    await page.keyboard.press("Control+Shift+Delete");
    await shot("10-confirm-clear");
    await page.keyboard.press("Escape");
    await page.evaluate(() => (window.__clipMock.control.copyError = "copying to the clipboard is not implemented yet (todo Phase 11)"));
    await page.keyboard.press("Control+Enter");
    await shot("11-copy-error");
    await page.keyboard.press("Control+,");
    await shot("12-settings");
    await page.locator('[data-slot="scroll-area-viewport"]').evaluate((e) => (e.scrollTop = 99999));
    await shot("13-settings-bottom");
    if (problems.length) console.log("console problems:", problems.slice(0, 3));
    await ctx.close();
  }
  return 0;
}

// ---------------------------------------------------------------------------

const server = await startServer();
const browser = await chromium.launch();
let code = 1;
try {
  if (mode === "keys") code = await keys(browser, server.url);
  else if (mode === "perf") code = await perf(browser, server.url);
  else if (mode === "shots") code = await shots(browser, server.url);
  else console.log("usage: node scripts/ui-check.mjs keys|perf|shots [--url URL] [--out DIR]");
} finally {
  await browser.close();
  await server.stop();
}
process.exit(code ? 1 : 0);

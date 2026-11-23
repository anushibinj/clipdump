// Browser checks of the UI against the fake backend (src/dev, `vite --mode mock`).
//
//   node scripts/ui-check.mjs keys    keyboard flow assertions (exit code 1 on a failure)
//   node scripts/ui-check.mjs tasks   keyboard and mouse flow of the Tasks view (exit code 1 on a failure)
//   node scripts/ui-check.mjs assistant   the task assistant against the fake LLM answers: quick wins, what next,
//                                     next step, draft, digest, links, tasks from a text, the quiet nudge
//   node scripts/ui-check.mjs perf    keystroke to rendered timings with 1,000 fake items
//   node scripts/ui-check.mjs a11y    accessibility audit in light and dark: ARIA roles and names of the
//                                     search view and settings, a live region for the result count,
//                                     dialog semantics, a visible focus ring on every Tab stop, text
//                                     contrast (WCAG AA) over every visible text of every state,
//                                     prefers-reduced-motion, target sizes (exit code 1 on a failure)
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

  // Enter copies too; it pastes only when "Paste on Enter" is on (default off)
  await reset();
  const w2 = await selectedId();
  await page.keyboard.press("Enter");
  await settle();
  const c2 = await lastCall("copy_item");
  check("Enter copies the selected item without paste while the setting is off", c2?.args.id === w2 && c2?.args.paste === false, JSON.stringify(c2));

  // turn "Paste on Enter" on through the settings screen
  await page.keyboard.press("Control+,");
  await settle();
  await page.locator("#auto-paste").click();
  await settle();
  check("the Paste on Enter switch is enabled and saves autoPasteOnEnter", (await lastCall("set_settings"))?.args.settings.autoPasteOnEnter === true);
  await page.keyboard.press("Escape");
  await settle();
  await page.locator("input[cmdk-input]").focus();
  const w3 = await selectedId();
  await page.keyboard.press("Enter");
  await settle();
  const c3 = await lastCall("copy_item");
  check("Enter copies and pastes once the setting is on", c3?.args.id === w3 && c3?.args.paste === true, JSON.stringify(c3));
  await page.keyboard.press("Control+Enter");
  await settle();
  const c4 = await lastCall("copy_item");
  check("Ctrl+Enter still only copies with the setting on", c4?.args.id === w3 && c4?.args.paste === false, JSON.stringify(c4));
  await page.keyboard.press("Alt+2");
  await settle();
  check("Alt+2 still only copies with the setting on", (await lastCall("copy_item"))?.args.paste === false);
  // the setting is read again when the bar is shown: after a reset of the backend it is off again
  await reset();
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await settle();
  await page.keyboard.press("Enter");
  await settle();
  check("Enter does not paste again after the setting was reset", (await lastCall("copy_item"))?.args.paste === false);

  // Alt+1 and Alt+3
  await reset();
  await page.keyboard.type("a");
  await settle();
  const ids2 = await rowIds();
  await page.keyboard.press("Alt+1");
  check("Alt+1 copies the first visible row", (await lastCall("copy_item"))?.args.id === ids2[0]);
  await page.keyboard.press("Alt+3");
  check("Alt+3 copies the third visible row", (await lastCall("copy_item"))?.args.id === ids2[2]);
  await reset();
  await page.keyboard.type("zzzzqq");
  await settle();
  const before = (await calls("copy_item")).length;
  await page.keyboard.press("Alt+5");
  check("Alt+5 with fewer rows does nothing", (await calls("copy_item")).length === before);

  // copy failure shows a message and the bar keeps working
  await reset();
  await page.evaluate(() => (window.__clipMock.control.copyError = "clipboard writing is not supported on this platform"));
  await page.keyboard.press("Control+Enter");
  await settle();
  const alert = page.locator('footer [role="alert"]');
  check("a failed copy shows an inline error", (await alert.count()) === 1 && /not supported/.test(await alert.innerText()));
  await page.keyboard.press("ArrowDown");
  check("the bar still works after a failed copy", (await page.locator('[role="option"][aria-selected="true"]').count()) === 1);

  // a failed auto-paste: the item is copied, the bar is gone, the message waits for the next open
  await reset();
  await page.evaluate(() => (window.__clipMock.control.copyError = "copied, but auto-paste failed: SendInput injected 0 of 4 key events"));
  await page.keyboard.press("Control+Enter");
  await page.waitForTimeout(5600);
  check("a failed paste message is not lost while the bar is hidden", (await alert.count()) === 1 && /^copied, but auto-paste failed/.test(await alert.innerText()));
  await page.evaluate(() => window.__clipMock.emit?.("bar:shown"));
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
  check("Esc then hides the bar, the pane is remembered open", (await calls("hide_bar")).length === hides0 + 1 && (await page.locator('aside[aria-label="Preview"]').count()) === 1);

  // settings
  await reset();
  await page.keyboard.press("Control+,");
  await settle();
  await page.getByRole("tab", { name: "Capture" }).click();
  await page.locator("#paused").click();
  await settle();
  check("the pause switch saves through set_settings", (await lastCall("set_settings"))?.args.settings.paused === true);
  await page.getByRole("tab", { name: "History" }).click();
  await page.locator("#max-items").fill("500");
  await page.locator("#max-items").press("Enter");
  await settle();
  check("a number field commits on Enter", (await lastCall("set_settings"))?.args.settings.maxItems === 500);
  await page.getByRole("tab", { name: "Capture" }).click();
  await page.locator("#max-text").fill("0");
  await page.locator("#max-text").blur();
  await settle();
  check("an invalid number falls back to the stored value", (await page.locator("#max-text").inputValue()) === "5");
  await page.getByRole("tab", { name: "General" }).click();
  check("every switch and the hotkey button are enabled", (await page.locator('button[role="switch"]:disabled').count()) === 0 && (await page.getByRole("button", { name: "Change" }).isEnabled()));
  await page.getByRole("tab", { name: "Data" }).click();
  check("the data folder button is enabled", await page.getByRole("button", { name: /Open/ }).isEnabled());
  check("no placeholder text about a later update remains", (await page.getByText("later update").count()) === 0 && (await page.getByText("Not available yet").count()) === 0);
  await page.getByRole("tab", { name: "General" }).click();
  await page.locator("#autostart").click();
  await settle();
  check("Start at login saves startAtLogin", (await lastCall("set_settings"))?.args.settings.startAtLogin === false);
  await page.locator("#pinned-bar").click();
  await settle();
  check("Pinned bar saves pinnedBar", (await lastCall("set_settings"))?.args.settings.pinnedBar === true);
  await page.getByRole("tab", { name: "Data" }).click();
  await page.getByRole("button", { name: /Open/ }).click();
  await settle();
  check("Open data folder calls open_data_folder", (await lastCall("open_data_folder")) !== undefined);
  await page.evaluate(() => { window.__clipMock.control.openFolderError = "could not open the file manager"; });
  await page.getByRole("button", { name: /Open/ }).click();
  await settle();
  check("a failed Open data folder shows the error", (await page.getByRole("alert").filter({ hasText: "could not open the file manager" }).count()) === 1);

  // hotkey recorder
  await page.getByRole("tab", { name: "General" }).click();
    check("the hotkey shows as key caps", (await page.getByTestId("hotkey-chord").innerText()).replace(/\s+/g, "") === "CtrlShiftSpace");
  await page.getByRole("button", { name: "Change" }).click();
  check("Change starts recording", (await page.getByTestId("hotkey-recording").count()) === 1);
  await page.keyboard.press("Escape");
  await settle();
  check("Esc cancels the recording and keeps settings open", (await page.getByTestId("hotkey-recording").count()) === 0 && (await page.locator("h2", { hasText: "Settings" }).count()) === 1);
  const setCalls0 = (await calls("set_settings")).length;
  await page.getByRole("button", { name: "Change" }).click();
  await page.keyboard.down("Control");
  await page.keyboard.down("Alt");
  check("held modifiers show live", (await page.getByTestId("hotkey-recording").innerText()).replace(/\s+/g, "") === "CtrlAlt");
  await page.keyboard.press("KeyK");
  await page.keyboard.up("Alt");
  await page.keyboard.up("Control");
  await settle();
  check("a recorded chord is sent as hotkey", (await lastCall("set_settings"))?.args.settings.hotkey === "Ctrl+Alt+K" && (await calls("set_settings")).length === setCalls0 + 1);
  check("the new chord is shown", (await page.getByTestId("hotkey-chord").innerText()).replace(/\s+/g, "") === "CtrlAltK");
  await page.getByRole("button", { name: "Change" }).click();
  await page.keyboard.press("Space");
  await settle();
  check("a key without a modifier is refused inline and sends nothing", (await page.getByTestId("hotkey-error").count()) === 1 && (await calls("set_settings")).length === setCalls0 + 1);
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Change" }).click();
  await page.keyboard.down("Meta");
  await page.keyboard.press("KeyL");
  await page.keyboard.up("Meta");
  await settle();
  const err = await page.getByTestId("hotkey-error").innerText();
  check("a chord another app owns shows the backend error inline", err.includes("taken by another app"), err);
  check("and the old chord stays", (await page.getByTestId("hotkey-chord").innerText()).replace(/\s+/g, "") === "CtrlAltK");
  check("the Esc and Ctrl+, keys of recording never closed settings", (await page.locator("h2", { hasText: "Settings" }).count()) === 1);
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

  // bar:shown focuses and selects the query, bar:hidden keeps the view
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
    "bar:hidden keeps the view the bar was left on (settings stay open for the next open)",
    (await page.locator("h2", { hasText: "Settings" }).count()) === 1,
  );
  await page.keyboard.press("Control+,");
  await settle();
  check("Ctrl+, goes back to the search", (await page.locator("input[cmdk-input]").count()) === 1);

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

  // the history was moved aside at startup: a notice that stays until dismissed
  await reset();
  const sentence = "The history database was damaged and could not be opened. The old file was moved to history.corrupt-1700000000.db in C:\\Users\\me\\AppData\\Local\\clipdump, and a new empty history was started.";
  await page.evaluate((m) => {
    window.__clipMock.control.recovery = m;
    window.__clipMock.emit("history:recovered");
  }, sentence);
  await settle();
  const rec = page.locator('footer [role="alert"]');
  check("history:recovered shows the recovery notice", (await rec.count()) === 1 && (await rec.innerText()).includes("history.corrupt-1700000000.db"));
  await page.evaluate(() => window.__clipMock.emit("bar:hidden"));
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await page.waitForTimeout(5600);
  check("the recovery notice does not time out", (await rec.count()) === 1);
  const hides1 = (await calls("hide_bar")).length;
  await page.keyboard.press("Escape");
  await settle();
  check("Esc dismisses the recovery notice without hiding the bar", (await rec.count()) === 0 && (await calls("hide_bar")).length === hides1);
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await settle();
  check("a dismissed recovery notice stays dismissed on the next open", (await rec.count()) === 0);
  await page.keyboard.press("Escape");
  await settle();
  check("the next Esc hides the bar again", (await calls("hide_bar")).length === hides1 + 1);

  // `--perf` support (Phase 14): silent without the flag, acks with it
  await reset();
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await page.keyboard.type("bu");
  await settle();
  check("no perf_ack without --perf", (await calls("perf_ack")).length === 0);
  await page.evaluate(() => (window.__clipMock.control.perf = true));
  await page.evaluate(() => window.__clipMock.emit("bar:shown")); // get_stats now says perf: the next show acks
  await settle();
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await page.keyboard.type("i");
  await settle();
  const acks = await calls("perf_ack");
  check("--perf: the painted ack follows bar:shown", acks.some((a) => a.args.kind === "barPainted"), JSON.stringify(acks));
  check("--perf: each search reports its round trip in ms", acks.some((a) => a.args.kind === "searchRtt" && typeof a.args.ms === "number" && a.args.ms >= 0 && a.args.ms < 1000), JSON.stringify(acks));

  check("no console errors or page errors", problems.length === 0, problems.slice(0, 3).join(" | "));
  console.log(failed === 0 ? "\nall checks passed" : `\n${failed} check(s) failed`);
  await page.context().close();
  return failed;
}


// ---------------------------------------------------------------------------
// tasks
// ---------------------------------------------------------------------------

async function tasks(browser, url) {
  const { page, problems } = await openPage(browser, url);
  let failed = 0;
  const check = (name, ok, detail = "") => {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `  (${detail})`}`);
    if (!ok) failed += 1;
  };
  const settle = () => page.waitForTimeout(120);
  const reset = async () => {
    await page.evaluate(() => window.__clipMock.reset());
    await page.waitForTimeout(150);
    // Like a hidden and shown bar (the task list loads when it is shown). The view is remembered,
    // so go back to the search by hand when the task list is open.
    await page.evaluate(() => {
      window.__clipMock.emit("bar:hidden");
      window.__clipMock.emit("bar:shown");
    });
    await settle();
    if ((await page.locator('[aria-label="Tasks"]').count()) > 0) await page.keyboard.press("Control+t");
    await settle();
  };
  const store = () => page.evaluate(() => window.__clipMock.tasks().map((t) => ({ ...t })));
  const byTitle = async (title) => (await store()).find((t) => t.title === title);
  const titles = () => page.locator('[aria-label="Tasks"] [role="option"]').evaluateAll((els) => els.map((e) => e.querySelector(".truncate")?.textContent));
  const sections = () => page.locator('[aria-label="Tasks"] h3').evaluateAll((els) => els.map((e) => e.textContent.replace(/\s+/g, " ").trim()));
  const selectedTitle = () => page.locator('[aria-label="Tasks"] [aria-selected="true"] .truncate').first().textContent();
  const input = page.locator('input[placeholder]').first();
  const openTasks = async () => {
    if ((await page.locator('[aria-label="Tasks"]').count()) === 0) await page.keyboard.press("Control+t");
    await page.waitForSelector('[aria-label="Tasks"]');
    await settle();
  };

  // the way in: a button that names what is in flight, and Ctrl+T
  const button = page.locator('button[title="Tasks (Ctrl+T)"]');
  check("the search view names the task touched last", (await button.textContent()).includes("Fix the flaky checkout test"));
  check("and how many more are in flight", (await button.textContent()).includes("+1"));
  await openTasks();
  check("Ctrl+T opens the task list", (await page.locator('[aria-label="Tasks"]').count()) === 1);
  check("the input is focused", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));

  // what shows: everything in flight, a few up next, the rest hidden
  check("sections are Now and Up next", JSON.stringify(await sections()) === JSON.stringify(["Now 2", "Up next 2"]), JSON.stringify(await sections()));
  check(
    "tasks that are not important are hidden",
    !(await titles()).includes("Update the onboarding doc") && !(await titles()).includes("Book the dentist"),
  );
  check("the summary counts what is in flight and done", (await page.locator("text=2 in flight").count()) === 1 && (await page.locator("text=1 done today").count()) === 1);
  check("a button offers the 4 hidden ones", (await page.locator("button", { hasText: "Show 4 hidden" }).count()) === 1);
  check("the task in flight touched last is first and selected", (await selectedTitle()) === "Fix the flaky checkout test");
  check("its note shows", (await page.locator("text=Fails only on the second run").count()) === 1);

  // Ctrl+H shows and hides
  await page.keyboard.press("Control+h");
  await settle();
  check("Ctrl+H shows the hidden ones and done", JSON.stringify(await sections()) === JSON.stringify(["Now 2", "Up next 2", "Later 4", "Done 1"]), JSON.stringify(await sections()));
  check("the button now says Show less", (await page.locator("button", { hasText: "Show less" }).count()) === 1);
  await page.keyboard.press("Control+h");
  await settle();
  check("Ctrl+H again hides them", JSON.stringify(await sections()) === JSON.stringify(["Now 2", "Up next 2"]));

  // arrows
  await page.keyboard.press("ArrowDown");
  check("ArrowDown selects the next task", (await selectedTitle()) === "Review the migration PR");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  // Up next is oldest first: the demo task has waited longest.
  check("ArrowUp wraps to the last", (await selectedTitle()) === "Reply to the vendor about the invoice");

  // capture
  await page.keyboard.type("Water the plants");
  await page.keyboard.press("Enter");
  await settle();
  const plants = await byTitle("Water the plants");
  check("Enter adds a waiting task that is not important", plants && plants.status === "todo" && plants.important === false);
  check("the input is cleared", (await input.inputValue()) === "");
  check("a task that would be hidden says where it went", (await page.locator("text=Ctrl+H to show them").count()) + (await page.locator("text=press Ctrl+H to show them").count()) >= 1);
  check("and does not show", !(await titles()).includes("Water the plants"));

  await page.keyboard.type("Call the bank !");
  await page.keyboard.press("Enter");
  await settle();
  const bank = await byTitle("Call the bank");
  check("a trailing ! marks it important and is not in the title", bank && bank.important === true && bank.status === "todo");

  await page.keyboard.type("Write the release notes");
  await page.keyboard.press("Control+Enter");
  await settle();
  const notes = await byTitle("Write the release notes");
  check("Ctrl+Enter adds and starts", notes && notes.status === "doing");
  check("it shows under Now and is selected", (await selectedTitle()) === "Write the release notes");
  check("the search view button follows what is in flight", await (async () => {
    await page.keyboard.press("Control+t");
    await settle();
    const t = await button.textContent();
    await page.keyboard.press("Control+t");
    await settle();
    return t.includes("Write the release notes") && t.includes("+2");
  })());

  await page.keyboard.type("ship it!");
  await page.keyboard.press("Enter");
  await settle();
  check("a ! stuck to a word stays in the title", !!(await byTitle("ship it!")) && (await byTitle("ship it!")).important === false);

  // blanks add nothing
  const count = (await store()).length;
  await page.keyboard.type("   ");
  await page.keyboard.press("Enter");
  await settle();
  check("blanks add nothing", (await store()).length === count);
  await input.fill("");

  // advance on an empty input
  await reset();
  await openTasks();
  check("after a reset the first task is selected again", (await selectedTitle()) === "Fix the flaky checkout test");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  check("selected a waiting task", (await selectedTitle()) === "Prepare the Thursday demo", await selectedTitle());
  await page.keyboard.press("Enter");
  await settle();
  check("Enter on an empty input starts the selected waiting task", (await byTitle("Prepare the Thursday demo")).status === "doing");
  check("it moved under Now", JSON.stringify(await sections()) === JSON.stringify(["Now 3", "Up next 1"]), JSON.stringify(await sections()));
  check("the selection follows it", (await selectedTitle()) === "Prepare the Thursday demo");
  await page.keyboard.press("Enter");
  await settle();
  const demo = await byTitle("Prepare the Thursday demo");
  check("Enter again finishes it", demo.status === "done" && demo.doneAt !== null);
  check("finishing says how to undo", (await page.locator("text=Shift+Enter puts it back").count()) === 1);
  check("and the done task leaves the calm view", !(await titles()).includes("Prepare the Thursday demo"));
  check("the neighbor is selected", (await selectedTitle()) !== null);
  // Undo: show the done ones; done tasks list the latest first, so it is the second row from the end.
  await page.keyboard.press("Control+h");
  await settle();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  check("the done task is selected", (await selectedTitle()) === "Prepare the Thursday demo", await selectedTitle());
  await page.keyboard.press("Shift+Enter");
  await settle();
  check("Shift+Enter steps a done task back to in flight", (await byTitle("Prepare the Thursday demo")).status === "doing");
  await page.keyboard.press("Shift+Enter");
  await settle();
  check("and once more back to waiting", (await byTitle("Prepare the Thursday demo")).status === "todo");
  await page.keyboard.press("Shift+Enter");
  await settle();
  check("a waiting task has no step back", (await byTitle("Prepare the Thursday demo")).status === "todo");

  // Ctrl+Enter on an empty input finishes directly
  await reset();
  await openTasks();
  await page.keyboard.press("Control+Enter");
  await settle();
  check("Ctrl+Enter on an empty input finishes the selected task", (await byTitle("Fix the flaky checkout test")).status === "done");
  // Ctrl+Enter on a done task reopens it: show everything, the last row is the oldest done one.
  await page.keyboard.press("Control+h");
  await settle();
  await page.keyboard.press("ArrowUp");
  check("the last row is a done task", (await selectedTitle()) === "Send the weekly summary", await selectedTitle());
  await page.keyboard.press("Control+Enter");
  await settle();
  check("Ctrl+Enter reopens a done task", (await byTitle("Send the weekly summary")).status === "todo");

  // important and edits
  await reset();
  await openTasks();
  await page.keyboard.press("Control+i");
  await settle();
  check("Ctrl+I toggles important", (await byTitle("Fix the flaky checkout test")).important === false);
  await page.keyboard.press("Control+i");
  await settle();
  check("and back", (await byTitle("Fix the flaky checkout test")).important === true);

  await page.keyboard.press("Control+e");
  await settle();
  check("Ctrl+E fills the input with the note", (await input.inputValue()) === "Fails only on the second run, check the seed data");
  check("and says it is editing a note", (await page.locator("text=Where did you leave off?").count()) >= 0 && (await input.getAttribute("placeholder")) === "Where did you leave off?");
  await input.fill("Found it: the seed runs twice");
  await page.keyboard.press("Enter");
  await settle();
  check("Enter saves the note", (await byTitle("Fix the flaky checkout test")).note === "Found it: the seed runs twice");
  check("and leaves the edit", (await input.inputValue()) === "" && (await input.getAttribute("placeholder")).startsWith("Add a task"));

  await page.keyboard.press("Control+e");
  await page.keyboard.type("x");
  await page.keyboard.press("Escape");
  await settle();
  check("Esc cancels an edit without saving", (await byTitle("Fix the flaky checkout test")).note === "Found it: the seed runs twice" && (await input.inputValue()) === "");
  check("and does not hide the bar", (await page.locator('[aria-label="Tasks"]').count()) === 1);

  await page.keyboard.press("F2");
  await input.fill("Fix the flaky payment test");
  await page.keyboard.press("Enter");
  await settle();
  check("F2 edits the title", !!(await byTitle("Fix the flaky payment test")));
  await page.keyboard.press("F2");
  await input.fill("  ");
  await page.keyboard.press("Enter");
  await settle();
  check("an empty title is refused", !!(await byTitle("Fix the flaky payment test")) && (await page.locator("text=needs a title").count()) >= 1);
  await page.keyboard.press("Escape");

  // Esc peels layers
  await page.keyboard.type("half typed");
  await page.keyboard.press("Escape");
  check("Esc clears typed text first", (await input.inputValue()) === "");
  await page.evaluate(() => (window.__clipMock.control.calls.length = 0));
  await page.keyboard.press("Escape");
  await settle();
  check("then Esc hides the bar", (await page.evaluate(() => window.__clipMock.control.calls.some((c) => c.cmd === "hide_bar"))) === true);

  // delete
  await reset();
  await openTasks();
  const before = (await store()).length;
  await page.keyboard.press("Control+Delete");
  await settle();
  check("Ctrl+Delete deletes the selected task", (await store()).length === before - 1 && !(await byTitle("Fix the flaky checkout test")));
  check("plain Delete is left to the input", await (async () => {
    await page.keyboard.type("abc");
    await page.keyboard.press("Home");
    await page.keyboard.press("Delete");
    const v = await input.inputValue();
    await input.fill("");
    return v === "bc" && (await store()).length === before - 1;
  })());

  // mouse
  await reset();
  await openTasks();
  await page.locator('[role="option"]', { hasText: "Prepare the Thursday demo" }).hover();
  await page.locator('button[aria-label^="Start: Prepare the Thursday demo"]').click();
  await settle();
  check("clicking the status circle starts a waiting task", (await byTitle("Prepare the Thursday demo")).status === "doing");
  check("and keeps focus in the input", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));
  await page.locator('[role="option"]', { hasText: "Prepare the Thursday demo" }).hover();
  await page.locator('[role="option"]', { hasText: "Prepare the Thursday demo" }).locator("button[aria-pressed]").click();
  await settle();
  check("clicking the star toggles important", (await byTitle("Prepare the Thursday demo")).important === false);
  await page.locator("button", { hasText: /Show \d+ hidden/ }).click();
  await settle();
  check("clicking Show hidden shows them", (await sections()).some((s) => s.startsWith("Later")));
  await page.locator("button", { hasText: "Search" }).first().click();
  await settle();
  check("the Search button goes back", (await page.locator('[aria-label="Tasks"]').count()) === 0 && (await page.locator("input[cmdk-input]").count()) === 1);

  // the bar being closed and opened again
  await openTasks();
  await page.keyboard.press("Control+h");
  await settle();
  check("the hidden ones are shown", (await sections()).some((s) => s.startsWith("Later")));
  await page.evaluate(() => window.__clipMock.emit("bar:hidden"));
  await settle();
  check("closing the bar keeps the task list open", (await page.locator('[aria-label="Tasks"]').count()) === 1);
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await settle();
  check("it is still the task list when the bar is shown again", (await page.locator('[aria-label="Tasks"]').count()) === 1 && (await page.locator("input[cmdk-input]").count()) === 0);
  check("the input is focused", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));
  check("the hidden ones are folded away again", (await sections()).every((s) => !s.startsWith("Later")));
  await page.keyboard.type("a draft");
  await page.evaluate(() => {
    window.__clipMock.emit("bar:hidden");
    window.__clipMock.emit("bar:shown");
  });
  await settle();
  check("a half typed task survives closing the bar", (await input.inputValue()) === "a draft");
  await input.fill("");

  // a restart: a new page load in the same browser profile
  await page.reload();
  await page.waitForSelector('[aria-label="Tasks"]');
  check("after a restart the app opens on the task list", (await page.locator('[aria-label="Tasks"]').count()) === 1);
  await page.keyboard.press("Control+t");
  await page.reload();
  await page.waitForSelector("input[cmdk-input]");
  check("and on the search when that was the last view", (await page.locator('[aria-label="Tasks"]').count()) === 0);
  check("blocked storage falls back to the search", await (async () => {
    await page.addInitScript(() => {
      Object.defineProperty(window, "localStorage", { get() { throw new Error("blocked"); } });
    });
    await page.reload();
    await page.waitForSelector("input[cmdk-input]");
    return true;
  })());

  // empty and failing
  await reset();
  await page.evaluate(() => window.__clipMock.setTasks([]));
  await openTasks();
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await settle();
  check("an empty list says how to start", (await page.locator("text=Nothing on the list").count()) === 1);
  check("the button reads Tasks when nothing is in flight", await (async () => {
    await page.keyboard.press("Control+t");
    await settle();
    return (await button.textContent()).trim().startsWith("Tasks");
  })());
  await page.evaluate(() => (window.__clipMock.control.tasksError = "the tasks are not available"));
  await page.keyboard.press("Control+t");
  await page.evaluate(() => window.__clipMock.emit("bar:shown"));
  await settle();
  check("an unavailable store says so, with the reason", (await page.getByText("Tasks are not available", { exact: true }).count()) === 1 && (await page.getByText("the tasks are not available", { exact: true }).count()) === 1);

  check("no console errors or page errors", problems.length === 0, problems.join(" | "));
  await page.context().close();
  console.log(failed === 0 ? "\nall checks passed" : `\n${failed} check(s) failed`);
  return failed === 0 ? 0 : 1;
}

// ---------------------------------------------------------------------------
// assistant
// ---------------------------------------------------------------------------

async function assistant(browser, url) {
  const { page, problems } = await openPage(browser, url);
  let failed = 0;
  const check = (name, ok, detail = "") => {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `  (${detail})`}`);
    if (!ok) failed += 1;
  };
  const settle = () => page.waitForTimeout(150);
  const reset = async () => {
    await page.evaluate(() => window.__clipMock.reset());
    await page.waitForTimeout(150);
    await page.evaluate(() => {
      window.__clipMock.emit("bar:hidden");
      window.__clipMock.emit("bar:shown");
    });
    await settle();
    if ((await page.locator('[aria-label="Tasks"]').count()) === 0) await page.keyboard.press("Control+t");
    await page.waitForSelector('[aria-label="Tasks"]');
    await settle();
  };
  const store = () => page.evaluate(() => window.__clipMock.tasks().map((t) => ({ ...t })));
  const byTitle = async (title) => (await store()).find((t) => t.title === title);
  const titles = () => page.locator('[aria-label="Tasks"] [role="option"]').evaluateAll((els) => els.map((e) => e.querySelector(".truncate")?.textContent));
  const sections = () => page.locator('[aria-label="Tasks"] h3').evaluateAll((els) => els.map((e) => e.textContent.replace(/\s+/g, " ").trim()));
  const selectedTitle = () => page.locator('[aria-label="Tasks"] [aria-selected="true"] .truncate').first().textContent();
  const panel = (name) => page.locator(`section[aria-label="${name}"]`);
  const calls = (cmd) => page.evaluate((c) => window.__clipMock.control.calls.filter((x) => x.cmd === c), cmd);
  const input = page.locator("input[placeholder]").first();
  // The selection survives a reset (the view stays mounted), so pick the task by walking the list.
  const selectTask = async (title) => {
    for (let i = 0; i < 12 && (await selectedTitle()) !== title; i += 1) await page.keyboard.press(i < 6 ? "ArrowUp" : "ArrowDown");
    return (await selectedTitle()) === title;
  };
  const hasText = async (loc, text) => ((await loc.count()) > 0 ? (await loc.first().textContent()).includes(text) : false);

  await reset();

  // --- the quiet nudge: the dentist has waited for 20 days -------------------------------------
  const nudge = panel("Gone quiet");
  check("a task that went quiet gets a nudge", (await nudge.count()) === 1 && (await hasText(nudge, "Book the dentist")));
  check("it says how long it has been quiet", await hasText(nudge, "Quiet for 2w"), await nudge.first().textContent());
  await nudge.getByRole("button", { name: "Ask the assistant" }).click();
  await settle();
  check("asking the assistant adds a suggestion", await hasText(nudge, "Suggests: Set it aside"), await nudge.first().textContent());
  check("and does not change the task", (await byTitle("Book the dentist")).deferredUntil === null);
  await nudge.getByRole("button", { name: "Set aside a week" }).click();
  await settle();
  const dentist = await byTitle("Book the dentist");
  check("Set aside a week defers it by seven days", dentist.deferredUntil > Date.now() + 6 * 86_400_000, String(dentist.deferredUntil));
  check("and the nudge goes away", (await nudge.count()) === 0);

  await reset();
  await nudge.getByRole("button", { name: "Drop" }).click();
  await settle();
  check("Drop deletes the task", (await byTitle("Book the dentist")) === undefined);
  await reset();
  await nudge.getByRole("button", { name: "Not now" }).click();
  await settle();
  const snoozed = await byTitle("Book the dentist");
  check("Not now asks again tomorrow", snoozed.deferredUntil > Date.now() && snoozed.deferredUntil < Date.now() + 2 * 86_400_000);
  check("the task itself still exists untouched", snoozed.status === "todo");

  // --- effort chips ----------------------------------------------------------------------------
  await reset();
  await page.keyboard.press("Control+h");
  await settle();
  const row = (title) => page.locator('[aria-label="Tasks"] [role="option"]', { hasText: title });
  check("an estimated task shows its size", await hasText(row("Rename the staging buckets"), "15 min"));
  check("a long one shows Long", await hasText(row("Prepare the Thursday demo"), "Long"));
  check("a waiting task shows the hourglass", (await row("Reply to the vendor about the invoice").locator('[title="Waiting"]').count()) === 1);
  check("an unestimated task shows no chip", !(await hasText(row("Book the dentist"), "min")) && !(await hasText(row("Book the dentist"), "Long")));
  await page.keyboard.press("Control+h");
  await settle();

  // --- quick wins (Ctrl+Q) ---------------------------------------------------------------------
  await page.keyboard.press("Control+q");
  await settle();
  check("Ctrl+Q shows only the quick wins", JSON.stringify(await titles()) === JSON.stringify(["Reply to the vendor about the invoice", "Rename the staging buckets"]), JSON.stringify(await titles()));
  check("in a Quick wins section", JSON.stringify(await sections()) === JSON.stringify(["Quick wins 2"]), JSON.stringify(await sections()));
  check("with the unestimated count in the summary", await hasText(page.locator("text=Quick wins ·"), "1 not estimated yet"));
  check("and an estimate was asked for the missing one", (await calls("estimate_backlog")).length === 1);
  await page.waitForTimeout(500);
  check("the estimate arrives by itself", (await byTitle("Book the dentist")).size !== null);
  check("the toolbar button shows how many, the new estimate included", await hasText(page.locator('button[aria-label="Quick wins"]'), "3"), await page.locator('button[aria-label="Quick wins"]').first().textContent());
  await page.keyboard.press("Control+q");
  await settle();
  check("Ctrl+Q again brings the calm view back", (await sections()).join("|") === "Now 2|Up next 2", (await sections()).join("|"));

  // a task added now gets its estimate a moment later (the background estimator)
  await input.fill("Pay the parking fine");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(600);
  check("a new task is estimated in the background", (await byTitle("Pay the parking fine")).size !== null);

  // --- what next (Ctrl+N) ----------------------------------------------------------------------
  await reset();
  await page.keyboard.press("Control+n");
  await page.waitForSelector('section[aria-label="What to do now"] p.font-medium');
  const pick = panel("What to do now");
  check("Ctrl+N shows a pick with a reason", (await hasText(pick, "Review the migration PR")) && (await hasText(pick, "most pressing")), await pick.first().textContent());
  await pick.getByRole("button", { name: "10 min" }).click();
  await page.waitForSelector('section[aria-label="What to do now"] p.font-medium');
  check("a short time asks again and prefers a quick task", await hasText(pick, "Reply to the vendor about the invoice"), await pick.first().textContent());
  check("and says why", await hasText(pick, "fits the time and energy"));
  check("the question carried the minutes", (await calls("pick_next_task")).at(-1).args.minutes === 10);
  await page.keyboard.press("Alt+Enter");
  await settle();
  check("Alt+Enter starts the picked task", (await byTitle("Reply to the vendor about the invoice")).status === "doing");
  check("and selects it", (await selectedTitle()) === "Reply to the vendor about the invoice");
  await page.keyboard.press("Escape");
  await settle();
  check("Esc closes the card and keeps the bar", (await pick.count()) === 0 && (await calls("hide_bar")).length === 0);

  // an unreachable server is a sentence with a retry, not a crash
  await page.evaluate(() => (window.__clipMock.control.llmError = "could not reach the LLM server (is it running, is the URL right?)"));
  await page.keyboard.press("Control+n");
  await page.waitForSelector('section[aria-label="What to do now"] [role="alert"]');
  check("a failure says why", await hasText(pick.locator('[role="alert"]'), "could not reach the LLM server"));
  await page.evaluate(() => (window.__clipMock.control.llmError = null));
  await pick.getByRole("button", { name: "Try again" }).click();
  await page.waitForSelector('section[aria-label="What to do now"] p.font-medium');
  check("Try again asks again", (await pick.locator('[role="alert"]').count()) === 0);
  await page.keyboard.press("Escape");

  // --- next step (Ctrl+G) ----------------------------------------------------------------------
  await reset();
  check("the selected task can be chosen by keyboard", await selectTask("Fix the flaky checkout test"));
  await page.keyboard.press("Control+g");
  await page.waitForSelector('section[aria-label="Next step"] ol');
  const steps = panel("Next step");
  check("Ctrl+G suggests a first step for the selected task", await hasText(steps, 'Open "Fix the flaky checkout test"'));
  check("and keeps it on the task", (await byTitle("Fix the flaky checkout test")).nextStep.startsWith("Open"));
  check("without counting as touching it", (await byTitle("Fix the flaky checkout test")).touchedAt < Date.now() - 600_000);
  check("it shows under the title", await hasText(row("Fix the flaky checkout test"), "Next:"));
  const before = (await store()).length;
  await page.keyboard.press("Alt+Enter");
  await settle();
  check("Alt+Enter adds the subtasks as tasks", (await store()).length === before + 3 && (await byTitle("Gather what you need")) !== undefined);
  check("and the card empties out", await hasText(steps, "Small enough to do as it is"));
  await steps.getByRole("button", { name: "Clear" }).click();
  await settle();
  check("Clear removes the suggested step", (await byTitle("Fix the flaky checkout test")).nextStep === "");
  await page.keyboard.press("Escape");

  // --- digest (Ctrl+D, Ctrl+Shift+D) -----------------------------------------------------------
  await reset();
  await page.keyboard.press("Control+d");
  await page.waitForSelector('section[aria-label="Digest"] p.leading-5');
  const digest = panel("Digest");
  check("Ctrl+D shows today's digest with an order", (await hasText(digest, "in flight")) && (await digest.locator("ol li").count()) === 3);
  await page.keyboard.press("Control+Shift+D");
  await page.waitForFunction(() => document.querySelector('section[aria-label="Digest"]')?.textContent.includes("this week") || document.querySelector('section[aria-label="Digest"]')?.textContent.includes("done this week"));
  check("Ctrl+Shift+D shows the week's recap", await hasText(digest, "done this week"));
  const askedBefore = (await calls("task_digest")).length;
  await digest.getByRole("button", { name: "Today" }).click();
  await settle();
  check("going back to a fresh digest reuses it", (await calls("task_digest")).length === askedBefore);
  await digest.locator("ol li button").first().click();
  await settle();
  check("clicking a task in the order selects it", (await selectedTitle()).length > 0);
  await page.keyboard.press("Escape");

  // --- tasks from a text (Ctrl+K) --------------------------------------------------------------
  await reset();
  await page.keyboard.press("Control+k");
  const extract = panel("Tasks from a text");
  check("Ctrl+K opens the box and focuses it", await page.evaluate(() => document.activeElement?.tagName === "TEXTAREA"));
  await page.keyboard.type("Hi team. Please send the signed lease to Dana by Friday. We need to book the venue. Also review the migration PR.");
  await page.keyboard.press("Control+Enter");
  await page.waitForSelector('section[aria-label="Tasks from a text"] [role="checkbox"]');
  check("Ctrl+Enter finds the action items", (await extract.locator('[role="checkbox"]').count()) === 3, String(await extract.locator('[role="checkbox"]').count()));
  check("the important one is marked", await hasText(extract, "important"));
  const ticks = await extract.locator('[role="checkbox"]').evaluateAll((els) => els.map((e) => e.getAttribute("aria-checked") === "true"));
  check("every new one starts ticked", ticks.every(Boolean), JSON.stringify(ticks));
  await extract.locator('[role="checkbox"]').nth(1).click();
  check("unticking one updates the button", (await extract.getByRole("button", { name: /^Add 2 tasks$/ }).count()) === 1);
  check("the box keeps the focus", await page.evaluate(() => document.activeElement?.tagName === "TEXTAREA"));
  await page.keyboard.press("Alt+Enter");
  await settle();
  check("Alt+Enter in the box adds only the ticked ones", (await byTitle("Please send the signed lease to Dana by Friday")) !== undefined && (await byTitle("We need to book the venue")) === undefined && (await byTitle("Also review the migration PR")) !== undefined);
  check("and keeps the importance", (await byTitle("Please send the signed lease to Dana by Friday")).important === true);
  check("then closes the card", (await extract.count()) === 0);
  check("and the focus is back in the input", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));

  // a duplicate is flagged and starts unticked
  await page.keyboard.press("Control+k");
  await page.keyboard.type("Review the migration PR.");
  await page.keyboard.press("Control+Enter");
  await page.waitForSelector('section[aria-label="Tasks from a text"] [role="checkbox"]');
  check("a task already on the list is flagged", await hasText(extract, "already on the list"));
  check("and starts unticked", (await extract.locator('[role="checkbox"]').first().getAttribute("aria-checked")) === "false");
  await page.keyboard.press("Escape");
  check("Esc inside the box closes the card", (await extract.count()) === 0);
  check("and gives the focus back to the input", await page.evaluate(() => document.activeElement?.tagName === "INPUT"));

  // what was just copied is used when the box is empty
  await page.evaluate(() => window.__clipMock.capture("Reminder: call the plumber about the leak tomorrow."));
  await settle();
  await page.keyboard.press("Control+k");
  await page.keyboard.press("Control+Enter");
  await page.waitForSelector('section[aria-label="Tasks from a text"] [role="checkbox"]');
  check("an empty box reads the newest copied text", await hasText(extract, "call the plumber about the leak"));
  await page.keyboard.press("Escape");

  // --- links (Ctrl+L) and the linked items in the notes ----------------------------------------
  await reset();
  await page.keyboard.press("Control+j");
  await page.waitForSelector('section[aria-label="Clipboard items for your tasks"] li');
  const links = panel("Clipboard items for your tasks");
  check("Ctrl+L suggests a clipboard item for a task", (await links.locator("li").count()) === 1 && (await hasText(links, "it looks related")));
  await links.getByRole("button", { name: /Link all/ }).click();
  await settle();
  const linked = (await store()).filter((t) => t.clipIds.length > 0);
  check("Link all stores the link on the task", linked.length === 1);
  check("and the row shows a paperclip with a count", (await row(linked[0].title).locator("svg.lucide-paperclip").count()) === 1);
  check("and the card empties out", await hasText(links, "Nothing you copied lately"));
  await page.keyboard.press("Escape");
  await row(linked[0].title).click();
  await page.keyboard.press("Control+e");
  await page.waitForSelector('[aria-label="Linked clipboard items"] li');
  check("the notes list the linked item", (await page.locator('[aria-label="Linked clipboard items"] li').count()) === 1);
  await page.locator('[aria-label="Linked clipboard items"] button[aria-label="Unlink"]').click();
  await settle();
  check("Unlink removes the link", (await store()).every((t) => t.clipIds.length === 0));
  check("and the list from the notes", (await page.locator('[aria-label="Linked clipboard items"]').count()) === 0);

  // --- draft where I left off (Ctrl+R) ---------------------------------------------------------
  await reset();
  await page.evaluate(() => window.__clipMock.capture("401 Unauthorized at /login on the second run"));
  await settle();
  await page.keyboard.press("Control+r");
  await page.waitForSelector('aside [role="status"]');
  const aside = page.locator("aside").first();
  check("Ctrl+R opens the notes with a draft", (await hasText(aside.locator('[role="status"]'), "401 Unauthorized")), await aside.locator('[role="status"]').first().textContent());
  check("the draft is not saved by itself", (await byTitle("Fix the flaky checkout test")).note.startsWith("Fails only"));
  await aside.getByRole("button", { name: "Use it" }).click();
  await page.waitForTimeout(700);
  check("Use it puts the draft in the note and saves it", (await byTitle("Fix the flaky checkout test")).note.includes("401 Unauthorized"));
  check("and the draft card goes away", (await aside.locator('[role="status"]').count()) === 0);
  await page.evaluate(() => (window.__clipMock.control.llmError = null));
  await page.keyboard.press("Escape");
  await page.keyboard.press("Control+Shift+End").catch(() => {});

  // --- set aside (Ctrl+B) and Esc order --------------------------------------------------------
  await reset();
  await page.keyboard.press("Control+b");
  await settle();
  const fixed = await byTitle("Fix the flaky checkout test");
  check("Ctrl+B sets the selected task aside for a week", fixed.deferredUntil > Date.now() + 6 * 86_400_000);
  check("and the row shows a clock", (await row("Fix the flaky checkout test").locator("svg.lucide-clock").count()) === 1);

  await reset();
  await page.keyboard.press("Control+d");
  await page.waitForSelector('section[aria-label="Digest"] p.leading-5');
  await input.fill("half typed");
  await page.keyboard.press("Escape");
  check("Esc clears the typed text first", (await input.inputValue()) === "" && (await panel("Digest").count()) === 1);
  await page.keyboard.press("Escape");
  check("then closes the card", (await panel("Digest").count()) === 0);
  check("and only then hides the bar", (await calls("hide_bar")).length === 0);
  await page.keyboard.press("Escape");
  await settle();
  check("the next Esc hides the bar", (await calls("hide_bar")).length === 1);

  // --- no LLM server set up --------------------------------------------------------------------
  await reset();
  await page.evaluate(() => {
    const s = window.__clipMock.settings();
    s.llmBaseUrl = "";
  });
  await page.keyboard.press("Control+n");
  await page.waitForSelector('section[aria-label="What to do now"] [role="alert"]');
  check("without a server the card says to set one up", await hasText(panel("What to do now"), "set up an LLM server in Settings first"));
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    window.__clipMock.settings().llmBaseUrl = "http://localhost:11434/v1";
  });

  check("no console errors or page errors", problems.length === 0, problems.join(" | "));
  await page.context().close();
  console.log(failed === 0 ? "\nall checks passed" : `\n${failed} check(s) failed`);
  return failed === 0 ? 0 : 1;
}

// ---------------------------------------------------------------------------
// a11y
// ---------------------------------------------------------------------------

/** Runs in the page: contrast of every visible text under `root`, against what is behind it. */
function contrastScan({ rootSelector, dark }) {
  const ctx = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const cache = new Map();
  // Any CSS color (oklch, color-mix, rgb) to [r, g, b, a] in sRGB, through a canvas pixel.
  const rgba = (css) => {
    if (cache.has(css)) return cache.get(css);
    ctx.clearRect(0, 0, 1, 1);
    ctx.fillStyle = "#000";
    ctx.fillStyle = css;
    ctx.fillRect(0, 0, 1, 1);
    const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
    const v = [r, g, b, a / 255];
    cache.set(css, v);
    return v;
  };
  const over = (top, under) => {
    const a = top[3];
    return [0, 1, 2].map((i) => top[i] * a + under[i] * (1 - a)).concat([1]);
  };
  const lum = ([r, g, b]) => {
    const f = (c) => {
      const s = c / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const ratio = (a, b) => {
    const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
  };
  // What is painted behind `el`: the nearest opaque background composited with the translucent
  // ones above it, over the window backdrop (white in light, black in dark: the worst case for a
  // translucent card on a transparent window).
  const backdrop = dark ? [0, 0, 0, 1] : [255, 255, 255, 1];
  const behind = (el) => {
    const layers = [];
    for (let n = el; n; n = n.parentElement) {
      const bg = rgba(getComputedStyle(n).backgroundColor);
      if (bg[3] > 0) layers.push(bg);
      if (bg[3] === 1) break;
    }
    return layers.reduceRight((under, top) => over(top, under), backdrop);
  };
  const opacityOf = (el) => {
    let o = 1;
    for (let n = el; n; n = n.parentElement) o *= Number(getComputedStyle(n).opacity);
    return o;
  };
  const describe = (el) => {
    const cls = typeof el.className === "string" ? el.className.split(/\s+/).slice(0, 3).join(".") : "";
    return `${el.tagName.toLowerCase()}${cls ? "." + cls : ""}`;
  };
  const out = [];
  const root = document.querySelector(rootSelector) ?? document.body;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const seen = new Set();
  for (let t = walker.nextNode(); t; t = walker.nextNode()) {
    const text = t.textContent.trim();
    const el = t.parentElement;
    if (!text || !el || seen.has(el)) continue;
    const box = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    if (box.width === 0 || box.height === 0 || cs.visibility === "hidden" || cs.display === "none") continue;
    if (el.closest("[hidden], [aria-hidden='true'], .sr-only")) continue;
    // A disabled control is exempt from the contrast requirement (WCAG 1.4.3).
    if (el.closest(":disabled, [aria-disabled='true']")) continue;
    seen.add(el);
    let fg = rgba(cs.color);
    const bg = behind(el);
    const op = opacityOf(el);
    fg = over([fg[0], fg[1], fg[2], fg[3] * op], bg);
    const size = parseFloat(cs.fontSize);
    const bold = Number(cs.fontWeight) >= 700;
    const large = size >= 24 || (bold && size >= 18.66);
    const need = large ? 3 : 4.5;
    const r = ratio(fg, bg);
    const where = el.closest("[role=option]") ? " in a row" : el.closest("footer") ? " in the footer" : "";
    out.push({ what: `${describe(el)} "${text.slice(0, 28)}"${where} fg ${fg.slice(0, 3).map(Math.round)} bg ${bg.slice(0, 3).map(Math.round)}`, ratio: Math.round(r * 100) / 100, need, ok: r >= need });
  }
  // Placeholders of text inputs.
  for (const input of root.querySelectorAll("input[placeholder]")) {
    const box = input.getBoundingClientRect();
    if (box.width === 0) continue;
    const bg = behind(input);
    const fg = over(rgba(getComputedStyle(input, "::placeholder").color), bg);
    const r = ratio(fg, bg);
    out.push({ what: `placeholder "${input.placeholder.slice(0, 28)}"`, ratio: Math.round(r * 100) / 100, need: 4.5, ok: r >= 4.5 });
  }
  return out;
}

async function a11y(browser, url) {
  let failed = 0;
  const check = (name, ok, detail = "") => {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `  (${detail})`}`);
    if (!ok) failed += 1;
  };

  for (const scheme of ["light", "dark"]) {
    console.log(`\n== ${scheme} ==`);
    const { page, ctx, problems } = await openPage(browser, url, { scheme });
    const settle = (ms = 150) => page.waitForTimeout(ms);
    const input = page.locator("input[cmdk-input]");
    const dark = scheme === "dark";

    // ---- page level ----------------------------------------------------------------
    check("the document has a language and a title", await page.evaluate(() => !!document.documentElement.lang && !!document.title));

    // ---- search view: combobox, listbox, options -----------------------------------------------
    await input.fill("build");
    await settle();
    const aria = await page.evaluate(() => {
      const i = document.querySelector("input[cmdk-input]");
      const lb = document.querySelector('[role="listbox"]');
      const active = i.getAttribute("aria-activedescendant");
      const opt = active ? document.getElementById(active) : null;
      return {
        role: i.getAttribute("role"),
        label: i.getAttribute("aria-label"),
        controls: i.getAttribute("aria-controls"),
        listboxId: lb?.id,
        listboxName: lb?.getAttribute("aria-label") || lb?.getAttribute("aria-labelledby"),
        expanded: i.getAttribute("aria-expanded"),
        activeRole: opt?.getAttribute("role"),
        activeSelected: opt?.getAttribute("aria-selected"),
        unnamedOptions: [...document.querySelectorAll('[role="option"]')].filter((o) => !(o.getAttribute("aria-label") || o.innerText).trim()).length,
        options: document.querySelectorAll('[role="option"]').length,
      };
    });
    check("the input is a combobox with a name", aria.role === "combobox" && !!aria.label, JSON.stringify(aria));
    check("the input controls the listbox", !!aria.controls && aria.controls === aria.listboxId, JSON.stringify(aria));
    check("the listbox has a name", !!aria.listboxName);
    check("aria-activedescendant points at the selected option", aria.activeRole === "option" && aria.activeSelected === "true");
    check("every option has a name", aria.options > 0 && aria.unnamedOptions === 0, `${aria.unnamedOptions} of ${aria.options}`);
    const before = await input.getAttribute("aria-activedescendant");
    await page.keyboard.press("ArrowDown");
    await settle(60);
    check("ArrowDown moves aria-activedescendant", (await input.getAttribute("aria-activedescendant")) !== before);

    // ---- live region for the result count -------------------------------------------------
    const live = page.locator('[aria-live="polite"]').first();
    const liveText = async () => (await live.count()) === 0 ? "" : (await live.innerText()).trim();
    await input.fill("build");
    await settle();
    const t1 = await liveText();
    await input.fill("zzzzqqq");
    await settle();
    const t2 = await liveText();
    await input.fill("");
    await settle();
    const t3 = await liveText();
    check("a polite live region announces the result count", /\d+ results?/.test(t1), JSON.stringify(t1));
    check("the announcement follows the query", t1 !== t2 && /no results/i.test(t2) && t3 !== t2, JSON.stringify([t1, t2, t3]));

    // ---- unnamed controls anywhere in the tree (search view) ---------------------------------
    const unnamed = async () => {
      const snap = await page.locator("body").ariaSnapshot();
      return snap.split("\n").filter((l) => /^\s*- (button|checkbox|switch|combobox|textbox|spinbutton|link|radio|option|tab|menuitem)(\s*\[|\s*:|\s*$)/.test(l));
    };
    check("every control in the search view has a name", (await unnamed()).length === 0, (await unnamed()).join(" | "));

    // ---- contrast: search states --------------------------------------------------------------
    const scan = async (label) => {
      const rows = await page.evaluate(contrastScan, { rootSelector: "main", dark });
      const bad = rows.filter((r) => !r.ok);
      check(`text contrast, ${label} (${rows.length} texts)`, bad.length === 0, bad.map((r) => `${r.what} ${r.ratio}<${r.need}`).slice(0, 4).join("; "));
    };
    await input.fill("");
    await settle();
    await scan("search, empty query");
    await input.fill("build");
    await settle();
    await scan("search, with matches");
    await input.fill("revenue");
    await settle();
    await scan("search, OCR match");
    await input.fill("");
    await page.keyboard.press("Alt+4");
    await settle();
    await scan("search, files with missing badge");
    await page.keyboard.press("Tab");
    await settle();
    await scan("search, files preview");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Alt+3");
    await page.keyboard.press("Tab");
    await settle();
    await scan("search, image preview");
    await page.keyboard.press("Tab");
    await input.fill("zzzzqqq");
    await settle();
    await scan("search, no matches");
    await input.fill("");
    await page.evaluate(() => (window.__clipMock.control.copyError = "clipboard writing is not supported on this platform"));
    await page.keyboard.press("Control+Enter");
    await settle();
    await scan("search, error notice");
    await page.evaluate(() => {
      window.__clipMock.control.copyError = null;
      window.__clipMock.control.recovery = "The history database was damaged and could not be opened. The old file was moved to history.corrupt-1700000000.db in C:\\Users\\me\\AppData\\Local\\clipdump, and a new empty history was started.";
      window.__clipMock.emit("history:recovered");
    });
    await page.getByRole("button", { name: "Dismiss" }).first().click();
    await settle(300);
    await scan("search, recovery notice");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Control+Shift+Delete");
    await settle();
    await scan("search, clear confirmation");
    await page.keyboard.press("Escape");

    // ---- tasks view: combobox and listbox like the search, names, contrast in every state ------------
    await page.evaluate(() => window.__clipMock.reset());
    await settle();
    await page.keyboard.press("Control+t");
    await settle(300);
    const tAria = await page.evaluate(() => {
      const i = document.querySelector('input[role="combobox"]');
      const lb = document.querySelector('[aria-label="Tasks"][role="listbox"]');
      const active = i?.getAttribute("aria-activedescendant");
      const opt = active ? document.getElementById(active) : null;
      const wrap = i?.parentElement;
      return {
        label: i?.getAttribute("aria-label"),
        controls: i?.getAttribute("aria-controls"),
        listboxId: lb?.id,
        activeRole: opt?.getAttribute("role"),
        activeSelected: opt?.getAttribute("aria-selected"),
        options: lb ? lb.querySelectorAll('[role="option"]').length : 0,
        unnamedOptions: lb ? [...lb.querySelectorAll('[role="option"]')].filter((o) => !o.innerText.trim()).length : -1,
        focused: document.activeElement === i,
        ring: wrap ? getComputedStyle(wrap).boxShadow !== "none" : false,
      };
    });
    check("the task input is a named combobox that controls the task list", !!tAria.label && !!tAria.controls && tAria.controls === tAria.listboxId, JSON.stringify(tAria));
    check("aria-activedescendant points at the selected task", tAria.activeRole === "option" && tAria.activeSelected === "true");
    check("every task has a name", tAria.options > 0 && tAria.unnamedOptions === 0, JSON.stringify(tAria));
    check("the task input has focus and shows a focus ring", tAria.focused && tAria.ring, JSON.stringify(tAria));
    check("every control in the task view has a name", (await unnamed()).length === 0, (await unnamed()).join(" | "));
    check("task sections are headings", (await page.getByRole("heading", { level: 3 }).count()) >= 2);
    await scan("tasks, calm view");
    await page.keyboard.press("Control+h");
    await settle();
    await scan("tasks, everything shown");
    await page.keyboard.press("Control+h");
    await page.keyboard.press("Control+e");
    await settle();
    await scan("tasks, editing a note");
    await page.keyboard.press("Escape");
    await page.keyboard.type("Plain task");
    await page.keyboard.press("Enter");
    await settle();
    await scan("tasks, notice about a hidden new task");

    // The assistant's cards, the quick wins view and the notes with a draft and a linked item.
    const card = async (keysToPress, selector, name) => {
      await page.keyboard.press(keysToPress);
      await page.waitForSelector(selector);
      await settle();
      await scan(name);
    };
    await card("Control+n", 'section[aria-label="What to do now"] p.font-medium', "tasks, what next card");
    check("every control with the card open has a name", (await unnamed()).length === 0, (await unnamed()).join(" | "));
    await page.keyboard.press("Escape");
    await card("Control+g", 'section[aria-label="Next step"] ol', "tasks, next step card");
    await page.keyboard.press("Escape");
    await card("Control+d", 'section[aria-label="Digest"] p.leading-5', "tasks, digest card");
    await page.keyboard.press("Escape");
    await card("Control+j", 'section[aria-label="Clipboard items for your tasks"] li', "tasks, links card");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Control+k");
    await page.keyboard.type("Please send the signed lease to Dana by Friday. We need to book the venue.");
    await card("Control+Enter", 'section[aria-label="Tasks from a text"] [role="checkbox"]', "tasks, tasks from a text card");
    check("every control in the text card has a name", (await unnamed()).length === 0, (await unnamed()).join(" | "));
    await page.keyboard.press("Escape");
    await card("Control+q", 'text=Quick wins', "tasks, quick wins");
    await page.keyboard.press("Control+q");
    await settle();
    await page.evaluate(() => window.__clipMock.capture("401 Unauthorized at /login on the second run"));
    await card("Control+r", 'aside [role="status"]', "tasks, notes with a draft");
    await page.keyboard.press("Escape");
    await page.evaluate(() => window.__clipMock.setTasks([]));
    await page.evaluate(() => window.__clipMock.emit("bar:shown"));
    await settle();
    await scan("tasks, empty list");
    await page.evaluate(() => (window.__clipMock.control.tasksError = "the tasks are not available"));
    await page.evaluate(() => window.__clipMock.emit("bar:shown"));
    await settle();
    await scan("tasks, unavailable");
    await page.evaluate(() => (window.__clipMock.control.tasksError = null));
    await page.keyboard.press("Control+t");
    await settle();

    // ---- settings: dialog semantics, names, focus ring on every Tab stop --------------------------
    await page.evaluate(() => window.__clipMock.reset());
    await settle();
    await page.keyboard.press("Control+,");
    await settle(300);
    const dlg = await page.evaluate(() => {
      const d = document.querySelector('[role="dialog"]');
      const name = d?.getAttribute("aria-label") || (d?.getAttribute("aria-labelledby") ? document.getElementById(d.getAttribute("aria-labelledby"))?.innerText : "");
      return { has: !!d, name, modal: d?.getAttribute("aria-modal"), inside: !!d?.contains(document.activeElement) };
    });
    check("settings is a modal dialog named Settings", dlg.has && /^Settings$/i.test(dlg.name ?? "") && dlg.modal === "true", JSON.stringify(dlg));
    check("focus is inside the settings dialog", dlg.inside);
    check("every control in settings has a name", (await unnamed()).length === 0, (await unnamed()).join(" | "));
    check("settings has headings", (await page.getByRole("heading", { name: "Settings" }).count()) === 1 && (await page.getByRole("tab").count()) === 5);

    const stops = [];
    for (let i = 0; i < 60; i += 1) {
      await page.keyboard.press("Tab");
      const st = await page.evaluate(() => {
        const e = document.activeElement;
        if (!e || e === document.body) return null;
        const cs = getComputedStyle(e);
        const wrap = e.closest("[data-slot=command-input-wrapper]");
        const w = wrap ? getComputedStyle(wrap) : null;
        const ring = (c) => (c.outlineStyle !== "none" && parseFloat(c.outlineWidth) > 0) || (c.boxShadow && c.boxShadow !== "none");
        return {
          id: e.id || e.getAttribute("aria-label") || e.textContent?.trim().slice(0, 20) || e.tagName,
          tag: e.tagName.toLowerCase(),
          ring: ring(cs) || (w ? ring(w) : false),
          inMain: !!e.closest("main"),
        };
      });
      if (!st) break;
      if (stops.length && stops[0].id === st.id && stops.length > 3) break;
      stops.push(st);
    }
    check("Tab reaches the settings controls", stops.length >= 8, `${stops.length} stops`);
    const noRing = stops.filter((s) => !s.ring);
    check("every Tab stop in settings shows a focus ring", noRing.length === 0, noRing.map((s) => s.id).join(", "));
    check("Tab stays inside the card", stops.every((s) => s.inMain));

    // The ring colour itself must be visible against the card (WCAG 1.4.11, 3:1).
    const ringRatio = await page.evaluate(
      ({ dark }) => {
        const ctx = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
        const px = (css) => {
          ctx.clearRect(0, 0, 1, 1);
          ctx.fillStyle = css;
          ctx.fillRect(0, 0, 1, 1);
          const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
          return [r, g, b, a / 255];
        };
        const rs = getComputedStyle(document.documentElement);
        const ring = px(rs.getPropertyValue("--ring"));
        const card = px(rs.getPropertyValue("--popover"));
        const base = dark ? [0, 0, 0] : [255, 255, 255];
        const bg = [0, 1, 2].map((i) => card[i] * card[3] + base[i] * (1 - card[3]));
        const fg = [0, 1, 2].map((i) => ring[i] * ring[3] + bg[i] * (1 - ring[3]));
        const lum = (c) => 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
        const f = (c) => ((c / 255) <= 0.03928 ? c / 255 / 12.92 : (((c / 255) + 0.055) / 1.055) ** 2.4);
        const [hi, lo] = [lum(fg), lum(bg)].sort((a, b) => b - a);
        return (hi + 0.05) / (lo + 0.05);
      },
      { dark },
    );
    check("the focus ring colour has 3:1 against the card", ringRatio >= 3, ringRatio.toFixed(2));

    await page.locator("main").focus();
    await scan("settings, top");
    await page.locator('[data-slot="scroll-area-viewport"]').evaluate((e) => (e.scrollTop = 99999));
    await settle();
    await scan("settings, bottom");
    await page.evaluate(() => {
      window.__clipMock.control.ocrUnavailable = true;
      window.__clipMock.control.hotkeyStartupError = "That shortcut is taken by another app or reserved by Windows. Choose a different one.";
    });
    await page.keyboard.press("Escape");
    await page.keyboard.press("Control+,");
    await settle(300);
    await scan("settings, hints and errors");
    await page.evaluate(() => (window.__clipMock.control.loginItemError = true));
    await page.locator("#autostart").click();
    await settle(200);
    check("a refused setting shows an alert banner", (await page.getByRole("alert").count()) >= 1);
    await scan("settings, save error banner");

    // ---- target sizes ----------------------------------------------------------------------------
    const small = await page.evaluate(() => {
      const bad = [];
      const sel = 'button, [role="switch"], [role="radio"], input, select, a[href]';
      for (const e of document.querySelectorAll(sel)) {
        const b = e.getBoundingClientRect();
        if (b.width === 0 || b.height === 0) continue;
        if (e.closest(".sr-only")) continue;
        if (b.width >= 24 && b.height >= 24) continue;
        // A switch is also toggled by its visible label (label for=id), a 44 px high row.
        const label = e.id ? document.querySelector(`label[for="${e.id}"]`) : null;
        if (label) {
          const lb = label.getBoundingClientRect();
          if (lb.height >= 24 && lb.width >= 24) continue;
        }
        bad.push(`${e.tagName.toLowerCase()} ${e.getAttribute("aria-label") || e.id || e.textContent.trim().slice(0, 16)} ${Math.round(b.width)}x${Math.round(b.height)}`);
      }
      return bad;
    });
    check("every control is at least 24x24 px (or has a larger label)", small.length === 0, small.join(", "));
    await page.keyboard.press("Escape");
    await page.evaluate(() => window.__clipMock.reset());

    // ---- reduced motion --------------------------------------------------------------------------
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.keyboard.press("Control+,");
    await settle(300);
    await page.locator("#auto-paste").click();
    await page.waitForTimeout(40);
    const motion = await page.evaluate(() => {
      const long = [];
      for (const e of document.querySelectorAll("*")) {
        const cs = getComputedStyle(e);
        const durations = cs.transitionDuration.split(",").concat(cs.animationDuration.split(",")).map((d) => parseFloat(d) * (d.includes("ms") ? 1 : 1000));
        if (durations.some((d) => d > 1)) long.push(`${e.tagName.toLowerCase()} ${cs.transitionDuration}/${cs.animationDuration}`);
      }
      return { long: [...new Set(long)].slice(0, 5), running: document.getAnimations().length };
    });
    check("prefers-reduced-motion removes transitions and animations", motion.long.length === 0, motion.long.join(", "));
    check("no animation is running with reduced motion", motion.running === 0, String(motion.running));
    await page.emulateMedia({ reducedMotion: "no-preference" });
    const moving = await page.evaluate(() => [...document.querySelectorAll("*")].some((e) => parseFloat(getComputedStyle(e).transitionDuration) > 0));
    check("without the preference the transitions are still there", moving);

    check("no console errors or page errors", problems.length === 0, problems.slice(0, 3).join(" | "));
    await ctx.close();
  }
  console.log(failed === 0 ? "\nall checks passed" : `\n${failed} check(s) failed`);
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
    const darkSet = ["02-text-search", "06-preview-image", "07-files-missing", "12-settings", "13-settings-bottom", "16-recovery-notice"];
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
    await page.evaluate(() => (window.__clipMock.control.copyError = "clipboard writing is not supported on this platform"));
    await page.keyboard.press("Control+Enter");
    await shot("11-copy-error");
    // The copy error above must not sit in the settings footer.
    await page.getByRole("button", { name: "Dismiss" }).click();
    await page.keyboard.press("Control+,");
    await shot("12-settings");
    // A chord another app owns: the refusal shows under the hotkey row.
    await page.getByRole("button", { name: "Change" }).click();
    await page.keyboard.down("Meta");
    await page.keyboard.press("KeyL");
    await page.keyboard.up("Meta");
    await shot("13-settings-hotkey-error");
    await page.locator('[data-slot="scroll-area-viewport"]').evaluate((e) => (e.scrollTop = 99999));
    await shot("13-settings-bottom");
    // The history was moved aside at startup: the notice stays until dismissed.
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.evaluate(() => {
      window.__clipMock.control.recovery =
        "The history database was damaged and could not be opened. The old file was moved to history.corrupt-1700000000.db in C:\\Users\\you\\AppData\\Local\\clipdump, and a new empty history was started.";
      window.__clipMock.emit("history:recovered");
    });
    await input.fill("");
    await shot("16-recovery-notice");
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
  else if (mode === "tasks") code = await tasks(browser, server.url);
  else if (mode === "assistant") code = await assistant(browser, server.url);
  else if (mode === "perf") code = await perf(browser, server.url);
  else if (mode === "a11y") code = await a11y(browser, server.url);
  else if (mode === "shots") code = await shots(browser, server.url);
  else console.log("usage: node scripts/ui-check.mjs keys|tasks|assistant|a11y|perf|shots [--url URL] [--out DIR]");
} finally {
  await browser.close();
  await server.stop();
}
process.exit(code ? 1 : 0);

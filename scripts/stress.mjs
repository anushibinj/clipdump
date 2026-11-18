// Stress and cold start runs of Phase 14 (docs/perf.md). Works on Windows and Linux; needs
// only Node and the Rust toolchain, and runs the same Rust harness on both
// (src-tauri/src/perf/stress.rs and restart.rs).
//
//   node scripts/stress.mjs                 the three stress scenarios, then the cold start measurement
//   node scripts/stress.mjs burst           10,000 entries at 50 per second, default settings
//   node scripts/stress.mjs full            10,000 entries as fast as the pipeline takes them
//   node scripts/stress.mjs capped          the same with a 256 MB disk cap
//   node scripts/stress.mjs cold            1,000 items (300 screenshots) opened cold and warm
//   node scripts/stress.mjs restart         the automated restart test (also part of `cargo test`)
//
// Options (also environment variables, see the top of perf/stress.rs for all of them):
//   --total N         entries per scenario (default 10000)
//   --image-pct N     percent of entries that are 4K screenshots (default 12)
//   --min-free-mb N   stop a run when the disk has less than this free (default 2048)
//   --dir PATH        where the temporary data folder goes (default: the system temp folder)
//   --out DIR         where the reports and CSV samples are written (default: <temp>/clipdump-stress)
//
// It builds with the `perf` Cargo profile (release without LTO, see src-tauri/Cargo.toml), which
// is what the figures in docs/perf.md come from. The shipping profile is fat LTO; real numbers are
// equal or better. Each run deletes its data folder. Exit code 1 if any run fails an invariant.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const opt = (name) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
};
const which = args.find((a, i) => !a.startsWith("--") && !(i > 0 && args[i - 1].startsWith("--"))) ?? "all";

const out = path.resolve(opt("out") ?? path.join(os.tmpdir(), "clipdump-stress"));
fs.mkdirSync(out, { recursive: true });

const env = { ...process.env };
const pass = (flag, name) => {
  const v = opt(flag);
  if (v !== undefined) env[`CLIPDUMP_STRESS_${name}`] = v;
};
pass("total", "TOTAL");
pass("image-pct", "IMAGE_PCT");
pass("min-free-mb", "MIN_FREE_MB");
pass("dir", "DIR");

const scenarios = {
  burst: "stress_burst_default_settings",
  full: "stress_full_speed_default_settings",
  capped: "stress_full_speed_capped_256mb",
  cold: "cold_start_1000_items_300_screenshots",
  restart: "restart_keeps_items_pins_and_ocr_search",
};
const plan = which === "all" ? ["burst", "full", "capped", "cold"] : [which];
for (const p of plan) {
  if (!scenarios[p]) {
    console.error(`unknown run "${p}". Choose: ${Object.keys(scenarios).join(", ")}, all`);
    process.exit(2);
  }
}

let failed = 0;
for (const name of plan) {
  const test = scenarios[name];
  const log = path.join(out, `${name}.txt`);
  console.log(`\n### ${name} (${test}) ...`);
  env.CLIPDUMP_STRESS_CSV = path.join(out, `${name}.csv`);
  const cargoArgs = ["test", "--profile", "perf", "--lib", test, "--", "--nocapture", "--test-threads=1"];
  if (name !== "restart") cargoArgs.splice(cargoArgs.indexOf("--nocapture"), 0, "--ignored");
  const r = spawnSync("cargo", cargoArgs, {
    cwd: path.join(repo, "src-tauri"),
    env,
    encoding: "utf8",
    maxBuffer: 1 << 30,
    shell: process.platform === "win32",
  });
  const text = `${r.stdout ?? ""}${r.stderr ?? ""}`;
  // The per item log lines are noise here; keep the reports and the verdict.
  const kept = text
    .split("\n")
    .filter((l) => !/^clipdump: (ingested|ocr: item|WARNING)/.test(l))
    .join("\n");
  fs.writeFileSync(log, kept);
  const report = kept.match(/=== (STRESS REPORT|COLD START REPORT)[\s\S]*?=== END[^\n]*===/);
  console.log(report ? report[0] : kept.split("\n").slice(-25).join("\n"));
  if (r.status !== 0) {
    failed += 1;
    console.log(`FAILED: ${name} (full output in ${log})`);
  } else {
    console.log(`ok: ${name} (full output in ${log})`);
  }
}
console.log(failed === 0 ? "\nall runs passed" : `\n${failed} run(s) failed`);
process.exit(failed === 0 ? 0 : 1);

// Fails when the app version is not the same everywhere, or when CHANGELOG.md has no
// heading for it. Run it as `pnpm check:version`. With a tag name (`v0.1.0` or
// `refs/tags/v0.1.0`) as the first argument, or in a GitHub Actions tag build
// (GITHUB_REF_TYPE is `tag`), it also checks that the tag is the version.
//
// Checked: package.json, src-tauri/Cargo.toml ([package] version), the clipdump entry of
// src-tauri/Cargo.lock, src-tauri/tauri.conf.json, and a `## [x.y.z]` heading in CHANGELOG.md.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(root, rel), "utf8");

/** Value of `version = "..."` in the first `[package]` table of a Cargo manifest. */
function cargoTomlVersion(text) {
  const table = text.match(/^\[package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m);
  const m = table && table[1].match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

/** Version of the `clipdump` package in a Cargo.lock. */
function cargoLockVersion(text) {
  const m = text.match(/^name = "clipdump"\r?\nversion = "([^"]+)"/m);
  return m ? m[1] : null;
}

const found = {
  "package.json": JSON.parse(read("package.json")).version ?? null,
  "src-tauri/Cargo.toml": cargoTomlVersion(read("src-tauri/Cargo.toml")),
  "src-tauri/Cargo.lock": cargoLockVersion(read("src-tauri/Cargo.lock")),
  "src-tauri/tauri.conf.json": JSON.parse(read("src-tauri/tauri.conf.json")).version ?? null,
};

const problems = [];
const expected = found["package.json"];
if (!expected) problems.push("package.json has no version");
for (const [file, version] of Object.entries(found)) {
  if (!version) problems.push(`${file}: no version found`);
  else if (expected && version !== expected) {
    problems.push(`${file} says ${version}, package.json says ${expected}`);
  }
}

if (expected) {
  const escaped = expected.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const heading = new RegExp(`^## \\[${escaped}\\]( - (\\d{4}-\\d{2}-\\d{2}|Unreleased))?\\s*$`, "m");
  if (!heading.test(read("CHANGELOG.md"))) {
    problems.push(`CHANGELOG.md has no "## [${expected}]" heading`);
  }
}

const fromCi = process.env.GITHUB_REF_TYPE === "tag" ? process.env.GITHUB_REF_NAME : "";
const tag = (process.argv[2] ?? fromCi ?? "").replace(/^refs\/tags\//, "");
if (tag && expected && tag !== `v${expected}`) {
  problems.push(`tag ${tag} is not v${expected}`);
}

if (problems.length > 0) {
  console.error("check-version failed:");
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`version ${expected}: package.json, Cargo.toml, Cargo.lock, tauri.conf.json and CHANGELOG.md agree${tag ? `, tag ${tag}` : ""}`);

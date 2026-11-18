# Environment and verification strategy

Recorded during Phase 0 (2026-11-18). The product targets Windows 10/11; the
development container used by Claude Code is Linux, so some checks can only be
done by the user on Windows.

## Phase 0 results (Linux dev container, Ubuntu 24.04)

| Command | Output |
|---|---|
| `rustc -V` | `rustc 1.97.0 (2d8144b78 2026-07-07)` |
| `cargo -V` | `cargo 1.97.0 (c980f4866 2026-06-30)` |
| `node -v` | `v22.23.5` |
| `pnpm -v` | `10.28.0` |

All four meet the Phase 0 requirements (Rust stable, Node 22 LTS, pnpm).

## What was installed in the container to enable verification

- `rustup target add x86_64-pc-windows-gnu x86_64-pc-windows-msvc` (std only).
- apt: `gcc-mingw-w64-x86-64` (C compiler and linker for the gnu target).
- apt: `libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev build-essential pkg-config`
  (Tauri v2 Linux prerequisites; pulls in libsoup-3.0 and GTK 3).

Network access to crates.io and the Ubuntu mirrors works through the proxy.

## What was probed and what works

Probes used throwaway crates outside the repo.

| Check | Result |
|---|---|
| Native `cargo check` of a Tauri v2 crate (tray-icon, global-shortcut, single-instance, autostart) | Works (about 1 min cold) |
| Native `cargo test` of a Tauri crate (builds, links, runs) | Works |
| `cargo check --target x86_64-pc-windows-gnu` with `windows` 0.62 (clipboard, DPAPI, `Media_Ocr`, `Graphics_Imaging`, `Storage_Streams` features) | Works |
| `cargo check --target x86_64-pc-windows-gnu` of the Tauri crate and the three plugins (compiles `webview2-com`, `tao`, etc.) | Works |
| `cargo build --target x86_64-pc-windows-gnu` of the Tauri crate (links a `.exe` via mingw) | Works, but the exe cannot be run here |
| `cargo check --target x86_64-pc-windows-msvc` for pure Rust crates (`windows`, Tauri and plugins) | Works (no linker needed for check) |
| `cargo check --target x86_64-pc-windows-msvc` with a C dependency (`rusqlite` feature `bundled`) | Fails: `cc-rs` cannot find `lib.exe` / `cl.exe` |
| `cargo check --target x86_64-pc-windows-gnu` with `rusqlite` bundled | Works (mingw gcc compiles SQLite) |
| Run Windows binaries (wine) | Not available (`wine` not installed; not worth it, see below) |

Notes:

- Use the **gnu** target for cross-checking Windows-only code in this container. The msvc target is
  only usable for crates without C build scripts, so do not rely on it. `cargo-xwin` was not needed
  or installed.
- A `cargo check` on the Windows target proves the code type-checks against the real Win32 and
  WinRT bindings. It does not prove runtime behavior.

## Cannot be verified here, user must verify on Windows

1. **MSVC toolchain**: `rustup show` reports `stable-x86_64-pc-windows-msvc` and Visual Studio 2022
   Build Tools with "Desktop development with C++" are installed. The product and the release CI
   build with MSVC; this container only checks against the gnu target.
2. **WebView2 runtime** is present (preinstalled on Windows 10/11). Needed to run `pnpm tauri dev`.
3. **Windows OCR language**: at least one language in Settings > Time & language > Language (English
   is usually present). Needed for Phase 7 OCR; `OcrEngine::TryCreateFromUserProfileLanguages()` returns
   none without it.
4. **Anything that touches the real OS at runtime**: clipboard capture and write, DPAPI key protect,
   Windows.Media.Ocr, SendInput auto-paste, foreground window tracking, global hotkey, tray,
   autostart, multi-monitor and DPI, WebView2 rendering, the NSIS installer, and all numbers in
   Section 4 of PLAN.md (latency, RAM, CPU, installer size). These are the "Check:" steps at the end
   of Phases 1, 2, 4, 7, 11, 14 and 15.

Suggested one-time user check on Windows:

```
rustc -V ; cargo -V ; node -v ; pnpm -v
rustup show        # default host should be x86_64-pc-windows-msvc
```

## Verification strategy for later phases (in this container)

1. **Keep logic platform independent.** Store, retention, cache, search, dedup, crypto, db, blobs,
   ingest (image decode, hashing, previews), settings and model code must not depend on Win32. Per
   PLAN.md rule 12, put OS code behind the traits in `platform/mod.rs`.
2. **Gate Windows code** with `#[cfg(windows)]` (module `platform/windows` and its `windows` crate
   dependency under `[target.'cfg(windows)'.dependencies]`). Provide in-memory or stub
   implementations for the traits on other platforms (fake `ClipboardWatcher`, fake `OcrEngine`, and
   a file-based key provider in place of DPAPI, test-only) so the app logic can be tested on Linux.
3. **Run natively on Linux**: `cargo test` (unit tests for everything platform independent, using
   temp dirs), `cargo clippy --all-targets`, and `cargo bench` for the search bench (a relative
   check only; absolute budgets must be confirmed on Windows).
4. **Cross-check the Windows modules**: `cargo check --target x86_64-pc-windows-gnu` (and
   `cargo clippy --target x86_64-pc-windows-gnu`) from `src-tauri`. Run it after every change to
   `platform/windows/*`, the Tauri setup, or `Cargo.toml`. Treat a failure here as a blocker for the
   phase.
5. **Frontend**: `pnpm install`, `pnpm tsc --noEmit`, `pnpm build` and `pnpm test` (vitest, pure
   logic only) all run on Linux. The UI also runs in a plain browser against a fake backend:
   `pnpm dev:mock` (Vite mode `mock`, see `src/dev`; the mode aliases `@tauri-apps/api/core` and
   `/event` to the fake, so nothing of it is in a production build). The preinstalled Playwright
   drives it: `node scripts/ui-check.mjs keys|perf|shots` (documented at the top of the script; run
   it from a scratch directory, not the repo root). `perf` numbers are relative (Chromium on Linux,
   JavaScript mock), never a Windows figure.
6. **Tauri on Linux**: `cargo check` and `cargo test` of the Tauri crate work natively because the
   webkit2gtk-4.1 stack is installed. `pnpm tauri dev` needs a display (use `xvfb-run` if a smoke
   test is wanted); the Linux build is not a product target and is not used to judge Windows
   behavior.
   Smoke test of the real app (Phase 13): the debug build has two environment variables,
   `CLIPDUMP_SMOKE_INSERT_TEXT` (insert one text item through the store) and
   `CLIPDUMP_SMOKE_QUIT_MS` (call `app.exit(0)`, the call the tray's Quit makes, after that many
   milliseconds). With `XDG_DATA_HOME` and `XDG_CONFIG_HOME` pointed at a temp folder:
   `CLIPDUMP_SMOKE_INSERT_TEXT=x CLIPDUMP_SMOKE_QUIT_MS=0 xvfb-run -a target/debug/clipdump`
   must exit 0 with `shutdown Completed`, and the next launch must log `loaded 1 items`. A garbage
   `clipdump/history.db` in the data folder must be moved to `history.corrupt-<ts>.db` and the
   recovery notice must show in the bar (run Vite on port 1420, start openbox, press the hotkey
   with `xdotool key ctrl+shift+space`, screenshot with ImageMagick's `import -window root`).
7. **Performance tooling (Phase 14)**: `docs/perf.md` has the figures and how each was measured.
   `cargo build --profile perf` and `cargo test --profile perf ...` use the `perf` Cargo profile
   (release without LTO, hot crates optimized; far cheaper than the shipping fat LTO profile, which
   this container cannot afford to build repeatedly). `node scripts/stress.mjs` runs the stress
   scenarios and the cold start measurement of `src-tauri/src/perf/`; the real app runs under Xvfb
   and openbox with `--perf` (or `CLIPDUMP_PERF=1`): build it with
   `cargo build --profile perf --features tauri/custom-protocol --bin clipdump` after `pnpm build`
   (the feature embeds `dist`, so no Vite server is needed), point `XDG_DATA_HOME` and
   `XDG_CONFIG_HOME` at temp folders, press the hotkey with `xdotool`, then
   `clipdump --perf-summary <data folder>/clipdump/perf.log`. Delete `src-tauri/target/perf` and
   `src-tauri/target/debug/incremental` afterwards: together they are several gigabytes. All of it
   gives relative Linux figures, never a Windows number.
8. **Per phase hand-off**: at the end of each phase that has a runtime "Check:" step, list the steps
   that were verified here (compile, unit tests, cross-check) separately from those the user must run
   on Windows. Do not claim a Windows runtime check as passed from this container.
9. **Commit gate**: before each commit that touches Rust, `cargo check` natively and
   `cargo check --target x86_64-pc-windows-gnu` must both pass.

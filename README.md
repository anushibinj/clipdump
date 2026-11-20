# Clipdump

An ultra-fast clipboard history for Windows: it records text, screenshots and copied files, and
finds any of them again from the keyboard (`Ctrl+Shift+Space`, type, `Ctrl+Enter`). Text inside
screenshots is searchable through Windows OCR. Everything stays on the machine, encrypted at rest.

Move the bar by dragging it (it remembers its place per monitor), pin it open or dismiss it with the
two icons top right, and, with an OpenAI compatible server set up in Settings, split a long text into
separately copyable items on the right pane's **AI split** tab, ready by the time you open the bar (`Ctrl+Shift+S`).

`Ctrl+T` switches the bar to a task list kept next to the history: capture a task with one line and
`Enter`, see everything in flight at a glance with a note on where you left off, and keep the tasks that
are not important out of sight until you ask for them (`Ctrl+H`).

Status: version 0.1.0, not released. The code is tested on Linux and cross-checked against the Windows
APIs; it has not been run on Windows yet (`docs/manual-checks.md` lists what is left to verify).
The plan is in `PLAN.md`.

## Install

Download `clipdump_<version>_x64-setup.exe` from the GitHub release. It installs for the current user
(no administrator prompt) and is not code signed yet, so Windows SmartScreen will warn. The uninstaller
offers "Also delete clipboard history". Details: `docs/packaging.md`.

## Build

Needs Rust (MSVC toolchain), Node 22 and pnpm, see `docs/environment.md`.

```
pnpm install
pnpm tauri dev      # run the app
pnpm tauri build    # NSIS installer in src-tauri/target/release/bundle/nsis
pnpm tsc --noEmit && pnpm test
cd src-tauri && cargo test
```

## Release

Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, add the
`CHANGELOG.md` entry, check with `pnpm check:version`, then push a tag `vX.Y.Z`. The
`release` workflow builds the installers for every platform and drafts the release. See `docs/packaging.md`.

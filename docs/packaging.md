# Packaging and release (Windows)

Status: the configuration and the uninstall hook are written and checked as far as Linux allows
(see "What is verified" at the end). **The installer has not been built, installed or uninstalled
on Windows.** Every claim about what the installer does on a real machine is still to be ticked in
`docs/manual-checks.md`, Phase 15.

## What the installer is

One NSIS installer, `clipdump_<version>_x64-setup.exe`, built by Tauri 2 (`tauri-cli` 2.12.5, which
uses `tauri-bundler` 2.10.5). The settings live in `src-tauri/tauri.conf.json`, section `bundle`:

| Setting | Value | Why |
|---|---|---|
| `targets` | `["nsis"]` | Only the NSIS installer is shipped. No MSI, so no WiX download and no per-machine install. |
| `windows.nsis.installMode` | `currentUser` | Per-user install, no administrator prompt. Metadata goes to `HKCU`. |
| `windows.nsis.languages` | `["English"]`, `displayLanguageSelector: false` | One language, no language dialog. |
| `windows.nsis.compression` | `lzma` | Smallest installer (the default, set explicitly because the budget is 12 MB). |
| `windows.nsis.installerIcon`, `uninstallerIcon` | `icons/icon.ico` | |
| `windows.nsis.startMenuFolder` | not set | The shortcut goes straight into Start, no `Clipdump` folder to tidy up. |
| `windows.webviewInstallMode` | `downloadBootstrapper`, `silent: true` | See "WebView2". |
| `windows.nsis.installerHooks` | `installer/hooks.nsh` | The uninstall hook, see "Uninstall". |
| `windows.nsis.customLanguageFiles` | `English` = `installer/English.nsh` | The label "Also delete clipboard history". |
| `publisher`, `copyright`, `category`, `shortDescription`, `longDescription` | `Clipdump`, `Copyright 2026 Clipdump contributors`, `Utility`, ... | Shown in "Apps and features" and in the file properties. |

The key names were taken from `node_modules/@tauri-apps/cli/config.schema.json` (the schema shipped
with the installed CLI), and `tauri.conf.json` validates against it with no error. Paths in the
`bundle` section are relative to `src-tauri`.

The Linux build and `pnpm tauri dev` are not affected: `targets` is only read by `tauri build`.

## Building the installer locally on Windows

Needs what Phase 0 of `PLAN.md` lists (Rust with the MSVC toolchain, Visual Studio Build Tools with
"Desktop development with C++", Node 22, pnpm 10), and internet access: the first build downloads NSIS
and the `nsis_tauri_utils` plugin into `%LOCALAPPDATA%\tauri`.

```
pnpm install --frozen-lockfile
pnpm check:version
pnpm tsc --noEmit
pnpm test
cd src-tauri; cargo test; cd ..
pnpm tauri build
```

`pnpm tauri build` runs `pnpm build` first (`beforeBuildCommand`), compiles the Rust side with the
`release` profile (fat LTO, one codegen unit: several minutes), and writes

```
src-tauri\target\release\clipdump.exe
src-tauri\target\release\bundle\nsis\clipdump_0.1.0_x64-setup.exe
```

The size of that second file is the "Installer size" row of `docs/perf.md` (budget: under 12 MB).

## Where it installs

Per-user, no administrator rights. The installer sets the default folder to
`%LOCALAPPDATA%\clipdump` (the template line is `StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"`, so
not `...\Programs\...`); the directory page lets the user pick another one. It also writes:

- the Start menu shortcut `%APPDATA%\Microsoft\Windows\Start Menu\Programs\clipdump.lnk`;
- a desktop shortcut when the installer runs silent (`/S`) or passive (`/P`); in the normal flow the
  finish page offers it as a checkbox. `/NS` skips shortcuts;
- the "Apps and features" entry under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\clipdump`;
- `HKCU\Software\Clipdump\clipdump` (install location, installer language).

**The install folder is also the data folder.** Clipdump keeps its history in
`%LOCALAPPDATA%\clipdump` (`app.path().local_data_dir()` plus `clipdump`, see `src-tauri/src/lib.rs`),
which is exactly the default install folder. They coexist: the installer and uninstaller only touch
their own files (`clipdump.exe`, `uninstall.exe`) and remove the folder with a plain, non-recursive
`RMDir`, which leaves it alone when data is in it. It also means the Windows "installed size" is
inflated by the history. If this ever matters, change the install folder in the template or move the
data folder (a data migration), not both silently.

Start at login is not written by the installer: on its first run the app registers itself with the
autostart plugin (value `clipdump` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, plus the
matching Task Manager switch under `...\Explorer\StartupApproved\Run`).

## WebView2

`webviewInstallMode` is `{ "type": "downloadBootstrapper", "silent": true }`. The installer checks the
registry for the Evergreen runtime (Windows 11 and current Windows 10 installs have it). If it is
missing, the installer downloads Microsoft's small bootstrapper and runs it with `/silent`. This keeps
the installer small, and needs internet access during install only on machines without the runtime.
The bootstrapper installs the runtime machine wide, so on such a machine Windows may show a UAC prompt
for that step even though Clipdump itself installs per user.

Offline alternatives, if the target machines have no internet access (change the `type` and rebuild):

- `{ "type": "embedBootstrapper", "silent": true }`: embeds the same small bootstrapper (about 1.8 MB
  more) in the installer. It still needs internet access to fetch the runtime, but not to fetch the
  bootstrapper.
- `{ "type": "offlineInstaller", "silent": true }`: embeds the full runtime installer, about 127 MB more.
  Works with no network; blows the 12 MB budget.
- `{ "type": "fixedRuntime", "path": "..." }`: ships a fixed runtime, about 180 MB more.
- `{ "type": "skip" }`: no runtime handling at all.

Whichever mode is used, the installer size in `docs/perf.md` must say which one it was measured with.

## Uninstall

Start the uninstaller from "Apps and features", or run `%LOCALAPPDATA%\clipdump\uninstall.exe`. The
confirm page has one checkbox:

> **Also delete clipboard history** (unchecked by default)

Unchecked: the program, the shortcuts, the "Apps and features" entry and the start at login entry are
removed. The history, the key and the settings stay, so a later reinstall finds everything.

Checked: additionally removes

| Path | What it holds |
|---|---|
| `%LOCALAPPDATA%\clipdump` | `history.db` (and `-wal`, `-shm`), `blobs\`, `thumbs\`, `key.bin` (the DPAPI protected key), `perf.log`, `history.corrupt-*` files |
| `%APPDATA%\clipdump` | `settings.json` |
| `%LOCALAPPDATA%\com.clipdump.app`, `%APPDATA%\com.clipdump.app` | the WebView2 profile (Tauri's own cleanup of the `identifier` folders) |
| `HKCU\Software\Clipdump\clipdump` | installer metadata (Tauri's own cleanup) |

Always, checked or not (but not for an update): the `clipdump` value under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and the `clipdump` value under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`.

A silent (`/S`) or passive (`/P`) uninstall never shows the confirm page, so the checkbox state stays
0 and **no data is removed**. There is no command line switch that deletes data; that is deliberate.
The uninstall step of an upgrade (`/UPDATE`) is also excluded, so upgrading never touches data or the
login entry. The running app is closed by the uninstaller before any file is removed.

How it is wired: Tauri's template shows the checkbox and stores its state in
`$DeleteAppDataCheckboxState`; the label comes from the language string `deleteAppData`.
`src-tauri/installer/English.nsh` is a copy of the bundler's `languages/English.nsh` with that one string
changed, and `src-tauri/installer/hooks.nsh` defines `NSIS_HOOK_POSTUNINSTALL`, which deletes the two
Clipdump folders when the state is 1. When the Tauri CLI is upgraded, diff `English.nsh` against the new
bundler's `languages/English.nsh` (crate `tauri-bundler`, `src/bundle/windows/nsis/languages/`) and copy
new strings over, and re-read the `Section Uninstall` of its `installer.nsi` to see that the variable
names used in `hooks.nsh` still exist.

## Release process

1. Decide the version `X.Y.Z`. Change it in `package.json`, `src-tauri/Cargo.toml` and
   `src-tauri/tauri.conf.json`; run `cargo check` once in `src-tauri` so `Cargo.lock` follows.
2. In `CHANGELOG.md`, move the entries of `[Unreleased]` under a new `## [X.Y.Z] - YYYY-MM-DD` heading
   and update the links at the bottom.
3. `pnpm check:version` must pass (it checks the three versions, `Cargo.lock` and the changelog heading).
4. Commit, push, wait for the checks you care about, then tag and push the tag:
   ```
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
5. The workflow `.github/workflows/release.yml` runs on the tag. A `checks` job on Windows runs install,
   `pnpm check:version`, a check that the tag is `vX.Y.Z`, `pnpm tsc --noEmit`, `pnpm test` and
   `cargo test`. Then a `build` matrix runs `pnpm tauri build --target <triple> --bundles <list>` for:
   Windows x64, x86 and arm64 (NSIS `.exe`), macOS Apple silicon and Intel (`.dmg`, the Intel one cross
   compiled), Linux x64 and arm64 (`.deb`, `.rpm`, `.AppImage`). Each leg lists its installer sizes in the
   run summary and uploads them as the artifact `clipdump-<name>`. A last job (the only one with
   `contents: write`) creates a **draft** GitHub release with every installer attached and generated
   notes. Open the draft, paste the changelog section, and publish it by hand. The macOS and Windows
   installers are unsigned.
6. Run the Phase 15 checklist in `docs/manual-checks.md` on a clean Windows machine or VM with the
   downloaded installer before announcing anything.

A manual run (Actions, "Run workflow") builds and uploads the artifact but creates no release.
The workflow was linted as YAML only; it has not run on GitHub yet, so expect to fix small things on
its first run.

## Code signing (future step)

**macOS:** the app is ad-hoc signed (`bundle.macOS.signingIdentity` is `"-"`), not signed with a Developer ID
or notarized. Apple silicon refuses a quarantined app with no signature at all ("clipdump is damaged"); an
ad-hoc signature turns that into the normal "unidentified developer" prompt, so users right-click the app, choose
Open, then Open again (or run `xattr -cr /Applications/clipdump.app`). For a clean install, enrol in the Apple
Developer Program and set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` in the release job; Tauri then signs and notarizes.

Nothing is signed today: no certificate is configured, no `signCommand`, no thumbprint. An unsigned
installer works, but Windows SmartScreen shows "Windows protected your PC / Unknown publisher" until the
file builds reputation, and some antivirus products are suspicious of unsigned installers. Users get past
it with "More info", then "Run anyway". Options when this is worth doing, none of them enabled:

- **Azure Trusted Signing**: cheapest managed option, no hardware token,
  needs an Azure subscription and an identity validation. Tauri calls a custom command per file, so use
  `bundle.windows.signCommand` with the `trusted-signing-cli` tool (`%1` is replaced by the file path) and
  put the Azure credentials in repository secrets.
- **An OV or EV code signing certificate** from a certificate authority. Since 2023 the private key must
  live on a hardware token or in a cloud HSM, so the CI job needs the vendor's cloud signing service or a
  self-hosted runner with the token. An EV certificate removes the SmartScreen warning at once; an OV one
  still has to build reputation. Use `bundle.windows.certificateThumbprint` (with `digestAlgorithm` set to
  `sha256` and a `timestampUrl`) when the certificate is in the Windows certificate store of the build
  machine, or `bundle.windows.signCommand` with `signtool sign /fd sha256 /tr <timestamp url> /td sha256 ...`
  for anything else.
- Either way, sign the installer and `clipdump.exe` (Tauri signs both, and the uninstaller, through the
  same setting), always add an RFC 3161 timestamp so signatures outlive the certificate, and keep keys and
  tokens in repository secrets, never in `tauri.conf.json`.

Signing changes the installer size slightly and must be re-checked against the 12 MB budget.

## What is verified, and what is not

Verified in the Linux container:

- `tauri.conf.json` is valid JSON and validates against the config schema of the installed CLI, with a
  negative control (a misspelled key and an unknown target are rejected).
- The real `installer.nsi` of `tauri-bundler` 2.10.5 (the version `tauri-cli` 2.12.5 pins), rendered by hand
  with this project's values (per-user mode, English, `installer/hooks.nsh`, `installer/English.nsh`), compiles
  with `makensis` 3.09 on Linux with the real `nsis_tauri_utils` plugin (SHA-1 matched the bundler's pin).
  The only warning is one the stock template also gives. The compiled installer contains the string
  "Also delete clipboard history".
- `pnpm check:version`, `pnpm tsc --noEmit`, `pnpm test`, `pnpm build`, `cargo test`, `cargo clippy`,
  `cargo fmt --check`, `cargo check --target x86_64-pc-windows-gnu`.

Not verifiable on Linux, to be done on Windows (`docs/manual-checks.md`, Phase 15): running `pnpm tauri
build`, the installer itself (install without UAC, install folder, shortcuts, the first launch), the
WebView2 bootstrapper, the uninstaller (both checkbox states, silent uninstall, the registry values), the
installer size, SmartScreen, and the workflow on GitHub.

## Upgrading over an existing install

`installer/installer.nsi` is a copy of tauri-bundler 2.10.5's NSIS template (wired in through
`bundle.windows.nsis.template`) with one change: when an older version is installed, the
pre-selected choice is **Update the existing installation**, which installs over it and keeps the
history and settings. The stock template pre-selects "Uninstall before installing" and runs the full
uninstaller. Uninstalling first is still offered as the second choice. Re-apply the patch (search for
`Clipdump:` in the file) and re-sync `English.nsh` when the Tauri CLI is upgraded. Not yet verified
on Windows: add to `docs/manual-checks.md`, Phase 15.

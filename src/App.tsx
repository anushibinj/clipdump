import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ConfirmInline } from "@/components/ConfirmInline";
import { EmptyState } from "@/components/EmptyState";
import { KbdHints } from "@/components/KbdHints";
import { Notice, type NoticeState } from "@/components/Notice";
import { PreviewPane } from "@/components/PreviewPane";
import { ResultList } from "@/components/ResultList";
import { rowId } from "@/components/ResultItem";
import { SearchBar } from "@/components/SearchBar";
import { SettingsView } from "@/components/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import { TypeFilter } from "@/components/TypeFilter";
import { useKeyboard } from "@/hooks/useKeyboard";
import { usePreview } from "@/hooks/usePreview";
import { useSearch } from "@/hooks/useSearch";
import {
  clearHistory,
  copyItem,
  deleteItem,
  errorMessage,
  getSettings,
  getStats,
  hideBar,
  onBarHidden,
  onBarShown,
  onHistoryChanged,
  onSettingsOpen,
  setSettings as saveSettingsDto,
  togglePin,
  type SettingsDto,
  type StatsDto,
} from "@/lib/ipc";
import type { Action, View } from "@/lib/shortcuts";
import { applyTheme, readThemePref, writeThemePref, type ThemePref } from "@/lib/theme";

/** How long "press again to clear" stays armed. */
export const CONFIRM_WINDOW_MS = 3000;
/** How long a footer message stays. */
const NOTICE_MS = 5000;

function App() {
  const s = useSearch();
  const [view, setView] = useState<View>("search");
  const [previewOpen, setPreviewOpen] = useState(false);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [stats, setStats] = useState<StatsDto | null>(null);
  const [notice, setNotice] = useState<NoticeState | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [themePref, setThemePref] = useState<ThemePref>(readThemePref);
  const [historyTick, setHistoryTick] = useState(0);

  const inputRef = useRef<HTMLInputElement>(null);
  const mainRef = useRef<HTMLElement>(null);
  const noticeTimer = useRef<number | undefined>(undefined);
  const stickyNotice = useRef(false);
  const confirmTimer = useRef<number | undefined>(undefined);

  // ---- latest values for the stable callbacks below -------------------------

  const latest = useRef({ ...s, settings, stats, view, previewOpen, confirming, themePref });
  latest.current = { ...s, settings, stats, view, previewOpen, confirming, themePref };

  // ---- footer messages ----------------------------------------------------

  // `sticky`: for a message raised while the bar is being hidden (nobody can read it yet).
  // It waits for the next `bar:shown` and only then starts its 5 seconds.
  const showNotice = useCallback((kind: NoticeState["kind"], message: string, sticky = false) => {
    window.clearTimeout(noticeTimer.current);
    setNotice({ kind, message });
    stickyNotice.current = sticky;
    if (!sticky) noticeTimer.current = window.setTimeout(() => setNotice(null), NOTICE_MS);
  }, []);

  const dismissNotice = useCallback(() => {
    window.clearTimeout(noticeTimer.current);
    stickyNotice.current = false;
    setNotice(null);
  }, []);

  const disarm = useCallback(() => {
    window.clearTimeout(confirmTimer.current);
    setConfirming(false);
  }, []);

  // ---- backend data -------------------------------------------------------

  const loadStats = useCallback(() => {
    getStats().then(setStats, () => {});
  }, []);
  const loadSettings = useCallback(() => {
    getSettings().then(setSettings, () => {});
  }, []);

  const saveSettings = useCallback(
    async (patch: Partial<SettingsDto>): Promise<string | null> => {
      const current = latest.current.settings;
      if (!current) return "Settings are not loaded yet.";
      try {
        setSettings(await saveSettingsDto({ ...current, ...patch }));
        loadStats();
        return null;
      } catch (e) {
        return errorMessage(e);
      }
    },
    [loadStats],
  );

  // ---- actions ------------------------------------------------------------

  // Whether plain Enter (and a click on a row) also pastes into the window the bar came
  // from: the "Paste on Enter" setting, off until the user turns it on. Ctrl+Enter and
  // Ctrl+1..9 never paste. Read through `latest` so the callbacks below stay stable.
  const pasteOnEnter = () => latest.current.settings?.autoPasteOnEnter ?? false;

  const copy = useCallback(
    (id: number, paste: boolean) => {
      // The backend hides the bar on success; on failure the bar stays open and the reason
      // is a footer message. "copied, but auto-paste failed: ..." means the item IS on the
      // clipboard (the bar is hidden by then), so it is kept until the bar is shown again.
      copyItem(id, paste).catch((e: unknown) => {
        const message = errorMessage(e);
        if (message.startsWith("copied, but")) showNotice("error", message, true);
        else showNotice("error", `Could not copy: ${message}`);
      });
    },
    [showNotice],
  );

  const onHover = s.select;
  const onActivate = useCallback(
    (id: number) => {
      latest.current.select(id);
      copy(id, pasteOnEnter());
    },
    [copy],
  );

  const run = useCallback(
    (a: Action) => {
      const c = latest.current;
      const selected = c.results.find((r) => r.id === c.selectedId) ?? null;
      switch (a.type) {
        case "move":
          c.move(a.delta);
          break;
        case "copy":
          if (selected) copy(selected.id, false);
          break;
        case "enter":
          if (selected) copy(selected.id, pasteOnEnter());
          break;
        case "copyNth": {
          const r = c.results[a.n - 1];
          if (r) copy(r.id, false);
          break;
        }
        case "togglePin":
          if (selected) {
            togglePin(selected.id).then(
              () => c.refresh(),
              (e: unknown) => showNotice("error", errorMessage(e)),
            );
          }
          break;
        case "delete":
          if (selected) {
            deleteItem(selected.id).then(
              () => c.refresh(),
              (e: unknown) => showNotice("error", errorMessage(e)),
            );
          }
          break;
        case "clear":
          if (c.confirming) {
            disarm();
            clearHistory().then(
              (n) => {
                c.refresh({ resetSelection: true });
                showNotice("info", `Cleared ${n} ${n === 1 ? "item" : "items"}, pinned items kept.`);
              },
              (e: unknown) => showNotice("error", errorMessage(e)),
            );
          } else {
            dismissNotice();
            setConfirming(true);
            window.clearTimeout(confirmTimer.current);
            confirmTimer.current = window.setTimeout(() => setConfirming(false), CONFIRM_WINDOW_MS);
            loadStats();
          }
          break;
        case "filter":
          c.setFilter(a.filter);
          break;
        case "togglePreview":
          setPreviewOpen((v) => !v);
          break;
        case "toggleSettings":
          disarm();
          setView((v) => (v === "settings" ? "search" : "settings"));
          break;
        case "escape":
          // Layers, innermost first: pending confirmation, settings, preview, then the bar.
          if (c.confirming) disarm();
          else if (c.view === "settings") setView("search");
          else if (c.previewOpen) setPreviewOpen(false);
          else hideBar().catch(() => {});
          break;
      }
    },
    [copy, dismissNotice, disarm, loadStats, showNotice],
  );

  const onKeyDownCapture = useKeyboard(view, run);

  // ---- events and first load ------------------------------------------------

  useEffect(() => {
    loadSettings();
    loadStats();
    const subs: Promise<UnlistenFn>[] = [];
    try {
      subs.push(
        onBarShown(() => {
          const input = inputRef.current;
          if (input) {
            input.focus();
            input.select();
          }
          latest.current.refresh({ resetSelection: true });
          if (stickyNotice.current) {
            stickyNotice.current = false;
            window.clearTimeout(noticeTimer.current);
            noticeTimer.current = window.setTimeout(() => setNotice(null), NOTICE_MS);
          }
          loadSettings(); // the tray can pause capture while the bar is hidden
          loadStats();
        }),
        onBarHidden(() => {
          // Back to the plain search view for the next open.
          setView("search");
          setPreviewOpen(false);
          disarm();
        }),
        onHistoryChanged(() => {
          latest.current.refresh();
          setHistoryTick((t) => t + 1);
          loadStats();
        }),
        onSettingsOpen(() => setView("settings")),
      );
    } catch {
      // No event bus (plain browser without the mock): the UI still works, it just never refreshes by itself.
    }
    return () => {
      subs.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [disarm, loadSettings, loadStats]);

  // Keep keyboard focus inside the card: the input in the search view, the card itself in
  // settings (so Esc and Ctrl+, still reach the key handler when no field is focused).
  useEffect(() => {
    if (view === "search") inputRef.current?.focus();
    else mainRef.current?.focus();
  }, [view]);

  const changeTheme = useCallback((pref: ThemePref) => {
    setThemePref(pref);
    writeThemePref(pref);
    applyTheme(pref);
  }, []);

  // ---- render ---------------------------------------------------------------

  const selected = s.results.find((r) => r.id === s.selectedId) ?? null;
  const preview = usePreview(s.selectedId, previewOpen && view === "search", historyTick);
  const unpinned = stats ? stats.items - stats.pinned : null;

  // The shortcut hints belong to the search view; settings shows the footer only for a message.
  const footer =
    view === "settings" && !notice ? null : (
      <footer className="flex h-9 shrink-0 items-center border-t px-3">
        {confirming ? (
          <ConfirmInline count={unpinned} windowMs={CONFIRM_WINDOW_MS} />
        ) : notice ? (
          <Notice {...notice} onDismiss={dismissNotice} />
        ) : (
          <KbdHints />
        )}
      </footer>
    );

  return (
    <TooltipProvider>
      {/* 8 px padding leaves room for the native window shadow and the card shadow. */}
      <div className="h-screen w-screen p-2">
        <main
          ref={mainRef}
          tabIndex={-1}
          onKeyDownCapture={view === "settings" ? onKeyDownCapture : undefined}
          className="h-full w-full overflow-hidden rounded-xl border outline-none border-border bg-popover/95 text-popover-foreground shadow-[0_8px_24px_-4px_rgb(0_0_0/0.22),0_2px_6px_rgb(0_0_0/0.1)] backdrop-blur-xl"
        >
          {view === "settings" ? (
            <div className="flex h-full flex-col">
              <div className="min-h-0 flex-1">
                <SettingsView
                  settings={settings}
                  stats={stats}
                  onSave={saveSettings}
                  themePref={themePref}
                  onThemeChange={changeTheme}
                  onClose={() => setView("search")}
                />
              </div>
              {footer}
            </div>
          ) : (
            <ResultList
              results={s.results}
              selectedId={s.selectedId}
              now={s.now}
              onHover={onHover}
              onActivate={onActivate}
              onKeyDownCapture={onKeyDownCapture}
              header={
                <>
                  <SearchBar
                    inputRef={inputRef}
                    query={s.query}
                    onQueryChange={s.setQuery}
                    itemCount={stats?.items ?? null}
                    paused={settings?.paused ?? false}
                    activeDescendant={s.selectedId === null ? undefined : rowId(s.selectedId)}
                  />
                  <TypeFilter value={s.filter} onChange={s.setFilter} />
                </>
              }
              empty={
                s.ready ? (
                  <EmptyState query={s.query} filter={s.filter} totalItems={stats?.items ?? null} error={s.error} />
                ) : null
              }
              aside={
                previewOpen ? (
                  <PreviewPane
                    result={selected}
                    preview={preview.preview}
                    error={preview.error}
                    ocrHint={stats?.ocrHint ?? ""}
                  />
                ) : null
              }
              footer={footer}
            />
          )}
        </main>
      </div>
    </TooltipProvider>
  );
}

export default App;

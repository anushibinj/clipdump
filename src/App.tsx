import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ConfirmInline } from "@/components/ConfirmInline";
import { EmptyState } from "@/components/EmptyState";
import { Notice, type NoticeState } from "@/components/Notice";
import { PreviewPane } from "@/components/PreviewPane";
import { ResultList } from "@/components/ResultList";
import { rowId } from "@/components/ResultItem";
import { SearchBar } from "@/components/SearchBar";
import { SettingsView } from "@/components/SettingsView";
import { ShortcutsButton, ShortcutsDialog } from "@/components/ShortcutsDialog";
import { TasksButton } from "@/components/TasksButton";
import { TasksView } from "@/components/TasksView";
import { TooltipProvider } from "@/components/ui/tooltip";
import { TypeFilter } from "@/components/TypeFilter";
import { useKeyboard } from "@/hooks/useKeyboard";
import { usePreview } from "@/hooks/usePreview";
import { useSearch } from "@/hooks/useSearch";
import { useSplit } from "@/hooks/useSplit";
import { useTasks } from "@/hooks/useTasks";
import { readView, writeView } from "@/lib/view";
import { readPaneOpen, readRightTab, writePaneOpen, writeRightTab, type RightTab } from "@/lib/pane";
import {
  clearHistory,
  copyItem,
  deleteItem,
  errorMessage,
  getSettings,
  getStats,
  afterPaint,
  hideBar,
  startDrag,
  onBarHidden,
  onBarShown,
  onHistoryChanged,
  onHistoryRecovered,
  onSettingsChanged,
  onSettingsOpen,
  perfAck,
  perfEnabled,
  setPerf,
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
  const tasks = useTasks();
  // The bar opens on the view it was left on (search, tasks or settings), across openings and
  // restarts: hiding the bar never resets it (`lib/view.ts`).
  const [view, setViewState] = useState<View>(readView);
  const setView = useCallback((next: View | ((v: View) => View)) => {
    setViewState((v) => {
      const to = typeof next === "function" ? next(v) : next;
      writeView(to);
      return to;
    });
  }, []);
  // The right pane stays open or closed the way it was left, across openings and restarts.
  const [previewOpen, setPreviewOpen] = useState(readPaneOpen);
  useEffect(() => writePaneOpen(previewOpen), [previewOpen]);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [stats, setStats] = useState<StatsDto | null>(null);
  const [notice, setNotice] = useState<NoticeState | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [themePref, setThemePref] = useState<ThemePref>(readThemePref);
  const [historyTick, setHistoryTick] = useState(0);
  // Counts the times the bar was hidden; the task list folds its revealed extras away on each.
  const [hiddenSignal, setHiddenSignal] = useState(0);
  // The sentence the user dismissed, so the recovery notice does not come back on the next open.
  const [recoveryDismissed, setRecoveryDismissed] = useState<string | null>(null);

  const inputRef = useRef<HTMLInputElement>(null);
  const mainRef = useRef<HTMLElement>(null);
  const noticeTimer = useRef<number | undefined>(undefined);
  const stickyNotice = useRef(false);
  const confirmTimer = useRef<number | undefined>(undefined);

  // ---- latest values for the stable callbacks below -------------------------

  // The history was moved aside at startup: a message that stays until dismissed (the 5 second
  // notices take the footer first, then it comes back).
  const recovery = stats?.recovery && stats.recovery !== recoveryDismissed ? stats.recovery : null;

  const latest = useRef({ ...s, settings, stats, view, previewOpen, confirming, themePref, recovery, shortcutsOpen });
  latest.current = { ...s, settings, stats, view, previewOpen, confirming, themePref, recovery, shortcutsOpen };

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
    getStats().then((st) => {
      setPerf(st.perf);
      setStats(st);
    }, () => {});
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

  // The right pane's tab (preview or AI split) is remembered across openings and restarts.
  const [rightTab, setRightTab] = useState<RightTab>(readRightTab);
  const changeTab = useCallback((tab: RightTab) => {
    setRightTab(tab);
    writeRightTab(tab);
  }, []);

  const run = useCallback(
    (a: Action) => {
      const c = latest.current;
      const selected = c.results.find((r) => r.id === c.selectedId) ?? null;
      // The shortcuts modal is on top: it only answers to its own key and Esc.
      if (c.shortcutsOpen && a.type !== "toggleShortcuts") {
        if (a.type === "escape") setShortcutsOpen(false);
        return;
      }
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
        case "split":
          // Ctrl+Shift+S: the right pane, on its AI split tab.
          changeTab("split");
          setPreviewOpen(true);
          break;
        case "toggleShortcuts":
          setShortcutsOpen((v) => !v);
          break;
        case "toggleSettings":
          disarm();
          setView((v) => (v === "settings" ? "search" : "settings"));
          break;
        case "toggleTasks":
          disarm();
          setView((v) => (v === "tasks" ? "search" : "tasks"));
          break;
        case "escape":
          // Layers, innermost first: pending confirmation, settings, the recovery notice (the
          // keyboard way to dismiss it), then the bar. The right pane is not a layer: it is
          // remembered, so Esc hides the bar and the pane is there again next time (Tab closes it).
          if (c.confirming) disarm();
          else if (c.view === "settings") setView("search");
          else if (c.recovery) setRecoveryDismissed(c.recovery);
          else hideBar().catch(() => {});
          break;
      }
    },
    [changeTab, copy, dismissNotice, disarm, loadStats, showNotice],
  );

  const onKeyDownCapture = useKeyboard(view, run);

  // ---- events and first load ------------------------------------------------

  const refreshTasks = tasks.refresh;
  useEffect(() => {
    loadSettings();
    loadStats();
    refreshTasks();
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
          // `--perf` only: tell the backend when the bar has been painted, for hotkey to paint.
          if (perfEnabled()) afterPaint(() => perfAck("barPainted"));
          if (stickyNotice.current) {
            stickyNotice.current = false;
            window.clearTimeout(noticeTimer.current);
            noticeTimer.current = window.setTimeout(() => setNotice(null), NOTICE_MS);
          }
          loadSettings(); // the tray can pause capture while the bar is hidden
          loadStats();
          refreshTasks();
        }),
        onBarHidden(() => {
          // The view stays as it is: the next open shows it again. Only a pending confirmation
          // and the task list's revealed extras go back to their calm state.
          setHiddenSignal((n) => n + 1);
          disarm();
        }),
        onHistoryChanged(() => {
          latest.current.refresh();
          setHistoryTick((t) => t + 1);
          loadStats();
        }),
        onSettingsOpen(() => setView("settings")),
        // The tray toggled Pause capture (the bar can stay visible in pinned mode).
        onSettingsChanged(() => loadSettings()),
        // The history was moved aside at startup; `get_stats` carries the sentence.
        onHistoryRecovered(() => loadStats()),
      );
    } catch {
      // No event bus (plain browser without the mock): the UI still works, it just never refreshes by itself.
    }
    return () => {
      subs.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [disarm, loadSettings, loadStats, refreshTasks]);

  // Keep keyboard focus inside the card: the input in the search and task views, the card itself
  // in settings (so Esc and Ctrl+, still reach the key handler when no field is focused).
  useEffect(() => {
    if (view !== "settings") inputRef.current?.focus();
    else mainRef.current?.focus();
  }, [view]);

  // The window has no title bar: the search row and the filter row are the drag handle. A press on
  // an input or a button keeps its own meaning.
  const dragFromHeader = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("input, button, a")) return;
    e.preventDefault();
    startDrag().catch(() => {});
  }, []);

  const togglePinnedWindow = useCallback(() => {
    const pinned = !latest.current.settings?.pinnedBar;
    saveSettings({ pinnedBar: pinned }).then((err) => {
      if (err) showNotice("error", err);
    });
  }, [saveSettings, showNotice]);

  const dismiss = useCallback(() => {
    hideBar().catch(() => {});
  }, []);

  const changeTheme = useCallback((pref: ThemePref) => {
    setThemePref(pref);
    writeThemePref(pref);
    applyTheme(pref);
  }, []);

  // ---- render ---------------------------------------------------------------

  const selected = s.results.find((r) => r.id === s.selectedId) ?? null;
  const paneOpen = previewOpen && view === "search";
  const preview = usePreview(s.selectedId, paneOpen && rightTab === "preview", historyTick);
  // The AI split starts by itself once its tab is showing a text item.
  const split = useSplit(s.selectedId, paneOpen && rightTab === "split" && selected?.kind === "text");
  const unpinned = stats ? stats.items - stats.pinned : null;
  const inFlight = useMemo(
    () => tasks.tasks.filter((t) => t.status === "doing").sort((a, b) => b.touchedAt - a.touchedAt),
    [tasks.tasks],
  );

  // The footer carries messages on the left and the Shortcuts button on the right.
  const footer = (
    <footer className="flex min-h-9 shrink-0 items-center gap-3 border-t px-3 py-1">
      {confirming ? (
        <ConfirmInline count={unpinned} windowMs={CONFIRM_WINDOW_MS} />
      ) : notice ? (
        <Notice {...notice} onDismiss={dismissNotice} />
      ) : recovery ? (
        <Notice kind="error" message={recovery} wrap onDismiss={() => setRecoveryDismissed(recovery)} />
      ) : null}
      <ShortcutsButton onClick={() => setShortcutsOpen(true)} />
    </footer>
  );

  // Announced politely by screen readers whenever it changes: the answer to "did my typing find
  // anything", which sighted users get from the list itself. Nothing before the first search.
  const resultCount = !s.ready
    ? ""
    : s.results.length === 0
      ? "No results"
      : `${s.results.length} ${s.results.length === 1 ? "result" : "results"}`;
  const inSettings = view === "settings";

  return (
    <TooltipProvider>
      {/* 8 px padding leaves room for the native window shadow and the card shadow. */}
      <div className="h-screen w-screen p-2">
        <main
          ref={mainRef}
          tabIndex={-1}
          // Settings replaces the list inside the same card: a modal dialog for assistive tech
          // (the card is the focus target, Esc leaves). The search view is the main landmark.
          role={inSettings ? "dialog" : undefined}
          aria-modal={inSettings ? true : undefined}
          aria-label={inSettings ? "Settings" : undefined}
          onKeyDownCapture={view !== "search" ? onKeyDownCapture : undefined}
          className="h-full w-full overflow-hidden rounded-xl border outline-none border-border bg-popover/95 text-popover-foreground shadow-[0_8px_24px_-4px_rgb(0_0_0/0.22),0_2px_6px_rgb(0_0_0/0.1)] backdrop-blur-xl"
        >
          <div role="status" aria-live="polite" aria-atomic="true" className="sr-only">
            {inSettings ? "" : resultCount}
          </div>
          {view === "tasks" ? (
            <div className="flex h-full flex-col">
              <div className="min-h-0 flex-1">
                <TasksView
                  api={tasks}
                  collapseSignal={hiddenSignal}
                  inputRef={inputRef}
                  onNotice={showNotice}
                  onBack={() => setView("search")}
                  onClose={dismiss}
                  onDragHeader={dragFromHeader}
                  pinned={settings?.pinnedBar ?? false}
                  onTogglePinned={togglePinnedWindow}
                />
              </div>
              {footer}
            </div>
          ) : view === "settings" ? (
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
                <div onMouseDown={dragFromHeader}>
                  <SearchBar
                    inputRef={inputRef}
                    query={s.query}
                    onQueryChange={s.setQuery}
                    itemCount={stats?.items ?? null}
                    paused={settings?.paused ?? false}
                    pinned={settings?.pinnedBar ?? false}
                    onTogglePinned={togglePinnedWindow}
                    onDismiss={dismiss}
                    activeDescendant={s.selectedId === null ? undefined : rowId(s.selectedId)}
                  />
                  <TypeFilter
                    value={s.filter}
                    onChange={s.setFilter}
                    trailing={<TasksButton inFlight={inFlight} onClick={() => setView("tasks")} />}
                  />
                </div>
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
                    tab={rightTab}
                    onTabChange={changeTab}
                    split={split}
                    onError={(m) => showNotice("error", m)}
                  />
                ) : null
              }
              footer={footer}
            />
          )}
        </main>
      </div>
      <ShortcutsDialog
        view={view}
        open={shortcutsOpen}
        onOpenChange={setShortcutsOpen}
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          (latest.current.view === "search" ? inputRef : mainRef).current?.focus();
        }}
      />
    </TooltipProvider>
  );
}

export default App;

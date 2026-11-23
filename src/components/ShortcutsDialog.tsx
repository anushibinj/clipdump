import { Settings } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import type { View } from "@/lib/shortcuts";

interface Shortcut {
  keys: string[];
  label: string;
}

const SHORTCUTS: Shortcut[] = [
  { keys: ["Enter"], label: "Copy (paste if enabled)" },
  { keys: ["Ctrl", "Enter"], label: "Copy" },
  { keys: ["Alt", "1-9"], label: "Copy the Nth result" },
  { keys: ["Up / Down"], label: "Move selection" },
  { keys: ["PgUp / PgDn"], label: "Move by a page" },
  { keys: ["Tab"], label: "Switch to tasks" },
  { keys: ["Alt", "P"], label: "Toggle preview" },
  { keys: ["Ctrl", "P"], label: "Pin or unpin" },
  { keys: ["Ctrl", "Shift", "S"], label: "Split text" },
  { keys: ["Ctrl", "Del"], label: "Delete item" },
  { keys: ["Ctrl", "Shift", "Del"], label: "Clear history" },
  { keys: ["Alt", "Shift", "1-4"], label: "Filter: all, text, images, files" },
  { keys: ["Ctrl", "1-3"], label: "Switch mode: clipboard, notes, tasks" },
  { keys: ["Alt", "/"], label: "Ask (also Alt+?)" },
  { keys: ["Ctrl", "T"], label: "Tasks" },
  { keys: ["Ctrl", "Shift", "N"], label: "Notes" },
  { keys: ["Ctrl", ","], label: "Settings" },
  { keys: ["Ctrl", "?"], label: "This help" },
  { keys: ["Esc"], label: "Close" },
];

const TASK_SHORTCUTS: Shortcut[] = [
  { keys: ["Enter"], label: "Add the typed task" },
  { keys: ["Ctrl", "Enter"], label: "Add and start it" },
  { keys: ["Enter"], label: "On an empty input: next step" },
  { keys: ["Shift", "Enter"], label: "On an empty input: step back" },
  { keys: ["Ctrl", "Enter"], label: "On an empty input: done" },
  { keys: ["Ctrl", "I"], label: "Important or not" },
  { keys: ["Ctrl", "E"], label: "Note: where you left off" },
  { keys: ["F2"], label: "Edit the title" },
  { keys: ["Ctrl", "H"], label: "Show or hide the hidden tasks" },
  { keys: ["Ctrl", "Q"], label: "Quick wins only (needs an LLM server)" },
  { keys: ["Ctrl", "N"], label: "What should I do now?" },
  { keys: ["Ctrl", "G"], label: "A concrete next step for the task" },
  { keys: ["Ctrl", "R"], label: "Draft where you left off" },
  { keys: ["Ctrl", "D"], label: "Digest of today" },
  { keys: ["Ctrl", "Shift", "D"], label: "Recap of the week" },
  { keys: ["Ctrl", "J"], label: "Link recent clipboard items to tasks" },
  { keys: ["Ctrl", "K"], label: "Find tasks in a text" },
  { keys: ["Ctrl", "B"], label: "Set the task aside for a week" },
  { keys: ["Alt", "Enter"], label: "Take the assistant's suggestion" },
  { keys: ["Ctrl", "L"], label: "Lock in on the task (focus mode)" },
  { keys: ["Ctrl", "P"], label: "Park a distraction while locked in" },
  { keys: ["Ctrl", "Del"], label: "Delete the task" },
  { keys: ["Tab"], label: "Back to the clipboard search" },
  { keys: ["Ctrl", "T"], label: "Back to the clipboard search" },
  { keys: ["Esc"], label: "Cancel an edit, clear the text, close" },
];

const NOTE_SHORTCUTS: Shortcut[] = [
  { keys: ["Ctrl", "Enter"], label: "New note" },
  { keys: ["Up / Down"], label: "Pick a note" },
  { keys: ["Ctrl", "G"], label: "Add tags to the note" },
  { keys: ["Ctrl", "P"], label: "Pin or unpin" },
  { keys: ["Ctrl", "Del"], label: "Delete the note (press twice)" },
  { keys: ["#tag"], label: "In the filter: only notes with the tag" },
  { keys: ["Ctrl", "E"], label: "Write in the picked note, or back to the filter" },
  { keys: ["Ctrl", "Shift", "N"], label: "Back to the clipboard search" },
  { keys: ["Esc"], label: "Clear the filter, close" },
];

const ASK_SHORTCUTS: Shortcut[] = [
  { keys: ["Enter"], label: "Ask the question" },
  { keys: ["Esc"], label: "Clear the text, then the answer, then close" },
];

/** The bottom right gear: opens settings. */
export const SettingsButton = memo(function SettingsButton({ onClick, active }: { onClick: () => void; active?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label="Settings"
      title="Settings (Ctrl+,)"
      aria-pressed={active}
      className="flex size-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
    >
      <Settings className="size-3.5" aria-hidden />
    </button>
  );
});

/** The bottom right trigger: opens the shortcuts help. */
export const ShortcutsButton = memo(function ShortcutsButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="ml-auto flex min-h-6 shrink-0 items-center gap-1.5 rounded-sm px-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
    >
      Shortcuts
      <span className="flex gap-0.5">
        <Kbd>Ctrl</Kbd>
        <Kbd>?</Kbd>
      </span>
    </button>
  );
});

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCloseAutoFocus?: (e: Event) => void;
  /** The view behind the dialog: its own shortcuts are listed first. */
  view?: View;
}

/** Shortcuts matching every word of the query, against the label and the keys. */
export function filterShortcuts(items: Shortcut[], query: string): Shortcut[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return items;
  return items.filter((s) => {
    const hay = `${s.label} ${s.keys.join(" ")}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

function List({ title, items }: { title: string; items: Shortcut[] }) {
  if (items.length === 0) return null;
  return (
    <section aria-label={title}>
      <h3 className="mb-1.5 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">{title}</h3>
      <ul className="flex flex-col gap-1.5 text-sm">
        {items.map((s) => (
          <li key={s.label} className="flex items-center justify-between gap-4">
            <span>{s.label}</span>
            <span className="flex shrink-0 gap-0.5">
              {s.keys.map((k) => (
                <Kbd key={k}>{k}</Kbd>
              ))}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** The "Shortcuts" modal: every keyboard shortcut in one list. */
export const ShortcutsDialog = memo(function ShortcutsDialog({ open, onOpenChange, onCloseAutoFocus, view = "search" }: Props) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  // A fresh search every time the dialog opens.
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);
  const clipboardItems = filterShortcuts(SHORTCUTS, query);
  const taskItems = filterShortcuts(TASK_SHORTCUTS, query);
  const noteItems = filterShortcuts(NOTE_SHORTCUTS, query);
  const askItems = filterShortcuts(ASK_SHORTCUTS, query);
  const noMatch = clipboardItems.length + taskItems.length + noteItems.length + askItems.length === 0;
  const clipboard = <List key="clipboard" title="Clipboard search" items={clipboardItems} />;
  const tasks = <List key="tasks" title="Tasks" items={taskItems} />;
  const notes = <List key="notes" title="Notes" items={noteItems} />;
  // The view behind the dialog comes first.
  const ask = <List key="ask" title="Ask" items={askItems} />;
  const sections =
    view === "tasks"
      ? [tasks, clipboard, notes, ask]
      : view === "notes"
        ? [notes, clipboard, tasks, ask]
        : view === "ask"
          ? [ask, clipboard, tasks, notes]
          : [clipboard, tasks, notes, ask];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-sm gap-3 p-4"
        aria-describedby={undefined}
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          inputRef.current?.focus();
        }}
        onCloseAutoFocus={onCloseAutoFocus}
      >
        <DialogHeader>
          <DialogTitle className="text-base">Shortcuts</DialogTitle>
          <DialogDescription className="sr-only">Keyboard shortcuts</DialogDescription>
        </DialogHeader>
        <Input
          ref={inputRef}
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search shortcuts"
          aria-label="Search shortcuts"
          autoComplete="off"
          spellCheck={false}
          className="h-8"
        />
        <div className="flex max-h-[60vh] flex-col gap-4 overflow-y-auto">
          {sections}
          {noMatch && <p className="py-4 text-center text-sm text-muted-foreground">No shortcuts match "{query}"</p>}
        </div>
      </DialogContent>
    </Dialog>
  );
});

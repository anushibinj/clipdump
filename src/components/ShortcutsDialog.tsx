import { memo } from "react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import type { View } from "@/lib/shortcuts";

interface Shortcut {
  keys: string[];
  label: string;
}

const SHORTCUTS: Shortcut[] = [
  { keys: ["Enter"], label: "Copy (paste if enabled)" },
  { keys: ["Ctrl", "Enter"], label: "Copy" },
  { keys: ["Ctrl", "1-9"], label: "Copy the Nth result" },
  { keys: ["Up / Down"], label: "Move selection" },
  { keys: ["PgUp / PgDn"], label: "Move by a page" },
  { keys: ["Tab"], label: "Toggle preview" },
  { keys: ["Ctrl", "P"], label: "Pin or unpin" },
  { keys: ["Ctrl", "Shift", "S"], label: "Split text" },
  { keys: ["Ctrl", "Del"], label: "Delete item" },
  { keys: ["Ctrl", "Shift", "Del"], label: "Clear history" },
  { keys: ["Alt", "1-4"], label: "Filter: all, text, images, files" },
  { keys: ["Ctrl", "T"], label: "Tasks" },
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
  { keys: ["Ctrl", "Del"], label: "Delete the task" },
  { keys: ["Ctrl", "T"], label: "Back to the clipboard search" },
  { keys: ["Esc"], label: "Cancel an edit, clear the text, close" },
];

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

function List({ title, items }: { title: string; items: Shortcut[] }) {
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
  const tasksFirst = view === "tasks";
  const clipboard = <List key="clipboard" title="Clipboard search" items={SHORTCUTS} />;
  const tasks = <List key="tasks" title="Tasks" items={TASK_SHORTCUTS} />;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm gap-3 p-4" aria-describedby={undefined} onCloseAutoFocus={onCloseAutoFocus}>
        <DialogHeader>
          <DialogTitle className="text-base">Shortcuts</DialogTitle>
          <DialogDescription className="sr-only">Keyboard shortcuts</DialogDescription>
        </DialogHeader>
        <div className="flex max-h-[60vh] flex-col gap-4 overflow-y-auto">{tasksFirst ? [tasks, clipboard] : [clipboard, tasks]}</div>
      </DialogContent>
    </Dialog>
  );
});

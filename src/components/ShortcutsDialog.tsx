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
  { keys: ["Tab"], label: "Switch to tasks" },
  { keys: ["Alt", "P"], label: "Toggle preview" },
  { keys: ["Ctrl", "P"], label: "Pin or unpin" },
  { keys: ["Ctrl", "Shift", "S"], label: "Split text" },
  { keys: ["Ctrl", "Del"], label: "Delete item" },
  { keys: ["Ctrl", "Shift", "Del"], label: "Clear history" },
  { keys: ["Alt", "1-4"], label: "Filter: all, text, images, files" },
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
  const clipboard = <List key="clipboard" title="Clipboard search" items={SHORTCUTS} />;
  const tasks = <List key="tasks" title="Tasks" items={TASK_SHORTCUTS} />;
  const notes = <List key="notes" title="Notes" items={NOTE_SHORTCUTS} />;
  // The view behind the dialog comes first.
  const sections = view === "tasks" ? [tasks, clipboard, notes] : view === "notes" ? [notes, clipboard, tasks] : [clipboard, tasks, notes];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm gap-3 p-4" aria-describedby={undefined} onCloseAutoFocus={onCloseAutoFocus}>
        <DialogHeader>
          <DialogTitle className="text-base">Shortcuts</DialogTitle>
          <DialogDescription className="sr-only">Keyboard shortcuts</DialogDescription>
        </DialogHeader>
        <div className="flex max-h-[60vh] flex-col gap-4 overflow-y-auto">{sections}</div>
      </DialogContent>
    </Dialog>
  );
});

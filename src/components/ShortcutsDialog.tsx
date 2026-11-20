import { memo } from "react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";

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
  { keys: ["Ctrl", ","], label: "Settings" },
  { keys: ["Ctrl", "?"], label: "This help" },
  { keys: ["Esc"], label: "Close" },
];

/** The bottom right trigger: opens the shortcuts help. */
export const ShortcutsButton = memo(function ShortcutsButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="ml-auto flex shrink-0 items-center gap-1.5 rounded-sm px-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
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
}

/** The "Shortcuts" modal: every keyboard shortcut in one list. */
export const ShortcutsDialog = memo(function ShortcutsDialog({ open, onOpenChange, onCloseAutoFocus }: Props) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm gap-3 p-4" aria-describedby={undefined} onCloseAutoFocus={onCloseAutoFocus}>
        <DialogHeader>
          <DialogTitle className="text-base">Shortcuts</DialogTitle>
          <DialogDescription className="sr-only">Keyboard shortcuts</DialogDescription>
        </DialogHeader>
        <ul className="flex flex-col gap-1.5 text-sm">
          {SHORTCUTS.map((s) => (
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
      </DialogContent>
    </Dialog>
  );
});

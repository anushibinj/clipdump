import { memo } from "react";
import { Kbd } from "@/components/ui/kbd";

interface Hint {
  keys: string[];
  label: string;
  /** Hide this hint first when the footer is narrow. */
  optional?: boolean;
}

const HINTS: Hint[] = [
  { keys: ["Ctrl", "Enter"], label: "copy" },
  { keys: ["Tab"], label: "preview" },
  { keys: ["Ctrl", "P"], label: "pin" },
  { keys: ["Ctrl", "Shift", "S"], label: "split", optional: true },
  { keys: ["Ctrl", "Del"], label: "delete", optional: true },
  { keys: ["Esc"], label: "close" },
];

/** The footer line of shortcut hints. */
export const KbdHints = memo(function KbdHints() {
  return (
    <ul className="@container flex w-full items-center gap-3.5 text-xs text-muted-foreground" aria-label="Keyboard shortcuts">
      {HINTS.map((h) => (
        <li
          key={h.label}
          className={h.optional ? "hidden items-center gap-1.5 @min-[560px]:flex" : "flex items-center gap-1.5"}
        >
          <span className="flex gap-0.5">
            {h.keys.map((k) => (
              <Kbd key={k}>{k}</Kbd>
            ))}
          </span>
          {h.label}
        </li>
      ))}
    </ul>
  );
});

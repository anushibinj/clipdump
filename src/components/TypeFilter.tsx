import { memo, type ReactNode } from "react";
import { Kbd } from "@/components/ui/kbd";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { TypeFilter as Filter } from "@/lib/ipc";
import { FILTER_ORDER } from "@/lib/shortcuts";

const LABELS: Record<Filter, string> = {
  all: "All",
  text: "Text",
  images: "Images",
  files: "Files",
};

interface TypeFilterProps {
  value: Filter;
  onChange: (value: Filter) => void;
  /** Right end of the row (the way into the task list). */
  trailing?: ReactNode;
}

/** All / Text / Images / Files, bound to Alt+Shift+1..4 (the key handling lives in useKeyboard). */
export const TypeFilter = memo(function TypeFilter({ value, onChange, trailing }: TypeFilterProps) {
  return (
    <div className="flex h-10 items-center justify-between gap-2 border-b px-3">
      <ToggleGroup
        type="single"
        size="sm"
        spacing={1}
        value={value}
        // Radix reports "" when the active item is pressed again; a filter is always on.
        onValueChange={(v) => v && onChange(v as Filter)}
        aria-label="Filter by type"
      >
        {FILTER_ORDER.map((f, i) => (
          <ToggleGroupItem
            key={f}
            value={f}
            tabIndex={-1}
            title={`${LABELS[f]} (Alt+Shift+${i + 1})`}
            className="h-7 min-w-0 gap-1.5 px-2.5 text-xs text-muted-foreground data-[state=on]:text-foreground"
            // Keep the caret in the search input when a filter is clicked.
            onMouseDown={(e) => e.preventDefault()}
          >
            {LABELS[f]}
            <Kbd className="h-4 min-w-4 px-1 text-[10px]">{i + 1}</Kbd>
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {trailing}
    </div>
  );
});

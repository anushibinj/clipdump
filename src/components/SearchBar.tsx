import type { Ref } from "react";
import { memo } from "react";
import { Badge } from "@/components/ui/badge";
import { CommandInput } from "@/components/ui/command";

interface SearchBarProps {
  query: string;
  onQueryChange: (value: string) => void;
  /** Total items in the history (not the number of results). */
  itemCount: number | null;
  paused: boolean;
  /** Id of the selected row, for `aria-activedescendant` (rows are not cmdk items, see ResultList). */
  activeDescendant?: string;
  inputRef: Ref<HTMLInputElement>;
}

/**
 * The search input row. The caller focuses and selects the text on `bar:shown`
 * through `inputRef`. The ring follows keyboard focus so it is always visible
 * where typing goes.
 */
export const SearchBar = memo(function SearchBar({
  query,
  onQueryChange,
  itemCount,
  paused,
  activeDescendant,
  inputRef,
}: SearchBarProps) {
  return (
    <CommandInput
      ref={inputRef}
      value={query}
      onValueChange={onQueryChange}
      placeholder="Search text, screenshots, files..."
      aria-label="Search clipboard history"
      aria-activedescendant={activeDescendant}
      autoFocus
      wrapperClassName="h-12 gap-2.5 px-4 transition-shadow has-[input:focus-visible]:shadow-[inset_0_-2px_0_0_var(--ring)]"
      className="h-11 text-[15px]"
      endAdornment={
        <div className="flex shrink-0 items-center gap-1.5">
          {paused && (
            <Badge
              variant="outline"
              className="border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400"
            >
              Paused
            </Badge>
          )}
          {itemCount !== null && (
            <Badge variant="secondary" className="tabular-nums">
              {itemCount} {itemCount === 1 ? "item" : "items"}
            </Badge>
          )}
        </div>
      }
    />
  );
});

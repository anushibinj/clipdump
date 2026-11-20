import { memo, useEffect, type RefObject } from "react";
import { Pin, PinOff, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CommandInput } from "@/components/ui/command";

interface SearchBarProps {
  query: string;
  onQueryChange: (value: string) => void;
  /** Total items in the history (not the number of results). */
  itemCount: number | null;
  paused: boolean;
  /** The window is pinned: it stays open on blur and after a copy. */
  pinned: boolean;
  onTogglePinned: () => void;
  /** Hide the bar (the same as Esc). */
  onDismiss: () => void;
  /** Id of the selected row, for `aria-activedescendant` (rows are not cmdk items, see ResultList). */
  activeDescendant?: string;
  inputRef: RefObject<HTMLInputElement | null>;
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
  pinned,
  onTogglePinned,
  onDismiss,
  activeDescendant,
  inputRef,
}: SearchBarProps) {
  // cmdk sets `aria-activedescendant` on the input itself from its own item store,
  // which is empty here (rows are not cmdk items), and that overrides a prop. Set it
  // on the element so screen readers follow the selected row.
  useEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    if (activeDescendant) input.setAttribute("aria-activedescendant", activeDescendant);
    else input.removeAttribute("aria-activedescendant");
  }, [inputRef, activeDescendant]);

  return (
    <CommandInput
      ref={inputRef}
      value={query}
      onValueChange={onQueryChange}
      placeholder="Search text, screenshots, files..."
      aria-label="Search clipboard history"
      autoFocus
      wrapperClassName="h-12 gap-2.5 px-4 transition-shadow has-[input:focus-visible]:border-ring has-[input:focus-visible]:shadow-[inset_0_-1px_0_0_var(--ring)]"
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
          {/* tabIndex -1 and no mousedown focus: typing and Tab (preview) stay with the input. */}
          <div className="ml-1 flex items-center gap-0.5">
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              tabIndex={-1}
              aria-label={pinned ? "Unpin window" : "Pin window open"}
              aria-pressed={pinned}
              title={pinned ? "Unpin: hide when focus is lost" : "Pin: keep the window open"}
              onMouseDown={(e) => e.preventDefault()}
              onClick={onTogglePinned}
              className={pinned ? "text-foreground" : "text-muted-foreground"}
            >
              {pinned ? <Pin className="fill-current" aria-hidden /> : <PinOff aria-hidden />}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              tabIndex={-1}
              aria-label="Dismiss"
              title="Dismiss (Esc)"
              onMouseDown={(e) => e.preventDefault()}
              onClick={onDismiss}
              className="text-muted-foreground"
            >
              <X aria-hidden />
            </Button>
          </div>
        </div>
      }
    />
  );
});

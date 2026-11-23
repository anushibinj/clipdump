import { memo, useEffect, type KeyboardEventHandler, type ReactNode } from "react";
import { Command, CommandList } from "@/components/ui/command";
import { ResultItem, rowId } from "@/components/ResultItem";
import type { ResultDto } from "@/lib/ipc";

interface ResultRowsProps {
  results: readonly ResultDto[];
  selectedId: number | null;
  now: number;
  onActivate: (id: number) => void;
}

/** The rows alone, memoized: typing re-renders the input but not this list unless results change. */
const ResultRows = memo(function ResultRows({
  results,
  selectedId,
  now,
  onActivate,
}: ResultRowsProps) {
  // Selection moved by keyboard: keep the row in view (nearest edge, no jump when visible).
  useEffect(() => {
    if (selectedId === null) return;
    document.getElementById(rowId(selectedId))?.scrollIntoView({ block: "nearest" });
  }, [selectedId]);

  return (
    <>
      {results.map((r, i) => (
        <ResultItem
          key={r.id}
          result={r}
          index={i}
          selected={r.id === selectedId}
          now={now}
          onActivate={onActivate}
        />
      ))}
    </>
  );
});

interface ResultListProps extends ResultRowsProps {
  /** Search input and filter row, above the list. */
  header: ReactNode;
  /** Shown instead of the rows when there are none. */
  empty: ReactNode;
  /** Preview pane, beside the list. */
  aside: ReactNode;
  footer: ReactNode;
  onKeyDownCapture: KeyboardEventHandler<HTMLElement>;
}

/**
 * The palette: a cmdk `Command` with `shouldFilter={false}` (the backend
 * ranks, cmdk must not filter or reorder), the input in `header`, the rows in
 * a `CommandList` (cmdk's `role="listbox"`), about eight of them visible.
 */
export function ResultList({
  header,
  empty,
  aside,
  footer,
  results,
  selectedId,
  now,
  onActivate,
  onKeyDownCapture,
}: ResultListProps) {
  return (
    <Command
      shouldFilter={false}
      vimBindings={false}
      loop
      label="Clipboard history"
      className="h-full rounded-xl bg-transparent"
      onKeyDownCapture={onKeyDownCapture}
      // A click on anything but the input or the preview text must not move focus away from typing.
      onMouseDown={(e) => {
        if (!(e.target as HTMLElement).closest("input, aside")) e.preventDefault();
      }}
    >
      {header}
      <div className="flex min-h-0 flex-1">
        <div className="@container min-w-0 flex-1">
          <CommandList className="h-full max-h-none py-1">
            {results.length === 0 ? (
              empty
            ) : (
              <ResultRows
                results={results}
                selectedId={selectedId}
                now={now}
                onActivate={onActivate}
              />
            )}
          </CommandList>
        </div>
        {aside}
      </div>
      {footer}
    </Command>
  );
}

import { useEffect, useState } from "react";
import { Kbd } from "@/components/ui/kbd";

interface ConfirmInlineProps {
  /** Unpinned items that would be removed. */
  count: number | null;
  /** How long the confirmation stays armed, in ms (a bar drains over this time). */
  windowMs: number;
}

/**
 * Footer text for "clear history": press Ctrl+Shift+Delete again within the
 * window to confirm. The caller owns the timer; this only shows the state and
 * a bar that drains over `windowMs` (a CSS transition, no animation library).
 */
export function ConfirmInline({ count, windowMs }: ConfirmInlineProps) {
  const [drained, setDrained] = useState(false);
  useEffect(() => {
    // Next frame, so the transition runs from full to empty.
    const raf = requestAnimationFrame(() => setDrained(true));
    return () => cancelAnimationFrame(raf);
  }, []);
  const what = count === null ? "the history" : `${count} ${count === 1 ? "item" : "items"}`;
  return (
    <div
      role="alert"
      className="relative flex w-full items-center gap-2 text-xs font-medium text-destructive"
    >
      <span>Press</span>
      <span className="flex gap-0.5">
        <Kbd>Ctrl</Kbd>
        <Kbd>Shift</Kbd>
        <Kbd>Delete</Kbd>
      </span>
      <span>again to clear {what} (pinned kept)</span>
      <span className="ml-auto text-muted-foreground">Esc cancels</span>
      <span
        aria-hidden
        className="absolute -bottom-2 left-0 h-0.5 rounded-full bg-destructive/70 ease-linear"
        style={{
          width: drained ? "0%" : "100%",
          transition: `width ${windowMs}ms linear`,
        }}
      />
    </div>
  );
}

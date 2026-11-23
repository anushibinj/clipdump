import { remainingMs, type FocusSession } from "@/lib/focus";

interface FocusProgressBarProps {
  /** The running focus session, or null when nothing is locked. */
  session: FocusSession | null;
  /** The shared once-a-second clock from `useFocus`. */
  now: number;
}

/**
 * A 1 px bar across the whole app, right below the title bar, that shows how much of the locked
 * task's time is left: full when the session starts, empty when it ends. Renders nothing when no
 * task is locked.
 */
export function FocusProgressBar({ session, now }: FocusProgressBarProps) {
  if (!session) return null;
  const left = Math.min(1, Math.max(0, remainingMs(session, now) / session.durationMs));
  return (
    <div
      role="progressbar"
      aria-label="Focus time left"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(left * 100)}
      className="h-px w-full shrink-0"
    >
      <div
        className="h-full origin-left bg-primary transition-transform duration-1000 ease-linear motion-reduce:transition-none"
        style={{ transform: `scaleX(${left})` }}
      />
    </div>
  );
}

import { useEffect, useState, type RefObject } from "react";
import { Coffee, Flame, Lock, Pause, Play, Timer } from "lucide-react";
import { ParkedList } from "@/components/ParkedList";
import { Button } from "@/components/ui/button";
import type { FocusApi } from "@/hooks/useFocus";
import {
  BREAK_FRICTION_MS,
  canPause,
  formatClock,
  pauseLeftMs,
  remainingMs,
  type ParkedItem,
} from "@/lib/focus";

interface FocusBannerProps {
  focus: FocusApi;
  /** The parking input, so Ctrl+P in the task input can focus it. */
  parkRef: RefObject<HTMLInputElement | null>;
  /** Give focus back to the task input (Esc in the parking input). */
  onReturnFocus: () => void;
  /** Check-in: the task is done. */
  onMarkDone: (taskId: number) => void;
  /** Check-in: open the picker for another session on the task. */
  onMoreTime: (taskId: number, title: string) => void;
  /** Check-in: blocked, so open the notes of the task to write down why. */
  onBlocked: (taskId: number) => void;
  /** Whether another session on this task makes sense (it exists and is not done). */
  canContinue: (taskId: number) => boolean;
  /** Turn a parked thought into a task. */
  onAddTask: (item: ParkedItem) => void;
}

/**
 * Everything about the focus lock that sits above the list: the countdown with its pause and
 * the slow way out, the parking lot, and what comes after a session (how it went, a break).
 * Renders nothing when there is nothing to show.
 */
export function FocusBanner(props: FocusBannerProps) {
  const { focus } = props;
  const { session, checkin, breakEndsAt, parked } = focus.data;
  if (session) return <Locked {...props} key={session.startedAt} />;
  if (checkin) return <CheckInPanel {...props} />;
  if (breakEndsAt !== null) return <BreakPanel focus={focus} />;
  if (parked.length > 0) return <ParkedReview {...props} />;
  return null;
}

const bar = "flex flex-col gap-2 border-b px-4 py-2.5 text-xs";

function StreakChip({ focus }: { focus: FocusApi }) {
  const { current, best } = focus.data.streak;
  return (
    <span
      className="flex shrink-0 items-center gap-1 text-muted-foreground tabular-nums"
      title={`Sessions in a row without breaking the lock. Best: ${best}`}
    >
      <Flame className="size-3.5" aria-hidden />
      {current}
    </span>
  );
}

/** A session is running: the list is locked. */
function Locked({ focus, parkRef, onReturnFocus, onAddTask }: FocusBannerProps) {
  const s = focus.data.session!;
  const left = remainingMs(s, focus.now);
  const paused = s.pausedAt !== null;
  const [text, setText] = useState("");
  const [showParked, setShowParked] = useState(false);
  // When the slow way out was started; null when it is not.
  const [armedAt, setArmedAt] = useState<number | null>(null);
  // `focus.now` ticks once a second, which is as fine as the countdown needs.
  const waited = armedAt === null ? 0 : Math.max(0, focus.now - armedAt);
  const ready = armedAt !== null && waited >= BREAK_FRICTION_MS;
  const parkedCount = focus.data.parked.length;

  // Resuming or finishing elsewhere never leaves a stale arming behind.
  useEffect(() => {
    if (!paused) return;
    setArmedAt(null);
  }, [paused]);

  const save = () => {
    if (text.trim() === "") return;
    focus.park(text);
    setText("");
  };

  return (
    <div className={`${bar} bg-primary/[0.06]`} role="region" aria-label="Focus lock">
      <div className="flex items-center gap-2.5">
        <Lock className="size-3.5 shrink-0 text-primary" aria-hidden />
        <span className="min-w-0 flex-1 truncate font-medium" title={s.title}>
          {s.title}
        </span>
        <span
          className="shrink-0 text-base font-semibold tabular-nums"
          role="timer"
          aria-label={`${formatClock(left)} left`}
        >
          {formatClock(left)}
        </span>
        <StreakChip focus={focus} />
        {paused ? (
          <Button type="button" variant="outline" size="xs" tabIndex={-1} onClick={focus.resume}>
            <Play aria-hidden />
            Resume ({formatClock(pauseLeftMs(s, focus.now))} of pause left)
          </Button>
        ) : (
          <Button
            type="button"
            variant="ghost"
            size="xs"
            tabIndex={-1}
            disabled={!canPause(s)}
            title={canPause(s) ? "Pause the clock for up to 5 minutes (once per session)" : "The pause is used"}
            onClick={focus.pause}
          >
            <Pause aria-hidden />
            Pause
          </Button>
        )}
      </div>

      <div className="flex items-center gap-2">
        <input
          ref={parkRef}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              e.stopPropagation();
              save();
            } else if (e.key === "Escape" || (e.ctrlKey && e.key.toLowerCase() === "p")) {
              e.preventDefault();
              e.stopPropagation();
              onReturnFocus();
            }
          }}
          maxLength={300}
          placeholder="Distracted by a thought? Park it here and press Enter (Ctrl+P)"
          aria-label="Park a distraction"
          spellCheck={false}
          className="h-7 min-w-0 flex-1 rounded-md border bg-background/60 px-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring"
        />
        {parkedCount > 0 && (
          <button
            type="button"
            tabIndex={-1}
            aria-expanded={showParked}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setShowParked((v) => !v)}
            className="shrink-0 rounded-md px-1.5 py-1 text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground"
          >
            {parkedCount} parked
          </button>
        )}
      </div>

      {showParked && parkedCount > 0 && (
        <ParkedList items={focus.data.parked} now={focus.now} onAddTask={onAddTask} onDismiss={focus.unpark} />
      )}

      <div className="flex items-center gap-2 text-muted-foreground">
        {armedAt === null ? (
          <button
            type="button"
            tabIndex={-1}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setArmedAt(Date.now())}
            className="underline-offset-2 hover:text-foreground hover:underline"
          >
            Break the lock…
          </button>
        ) : (
          <>
            <span className="tabular-nums" role="status">
              {ready
                ? "Sure? Your streak starts over."
                : `Breaking the lock in ${formatClock(BREAK_FRICTION_MS - waited)}…`}
            </span>
            {ready && (
              <Button type="button" variant="destructive" size="xs" tabIndex={-1} onClick={focus.breakLock}>
                Break the lock now
              </Button>
            )}
            <Button type="button" size="xs" tabIndex={-1} onClick={() => setArmedAt(null)}>
              Keep focusing
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

/** The session ended (or the task was finished early): how did it go, and what next. */
function CheckInPanel({ focus, onMarkDone, onMoreTime, onBlocked, canContinue, onAddTask }: FocusBannerProps) {
  const c = focus.data.checkin!;
  const { parked } = focus.data;
  return (
    <div className={`${bar} bg-primary/[0.06]`} role="region" aria-label="Focus session over">
      {c.stage === "ask" ? (
        <>
          <div className="flex items-center gap-2">
            <Timer className="size-3.5 shrink-0 text-primary" aria-hidden />
            <span className="min-w-0 flex-1 truncate">
              Time is up on <span className="font-medium">“{c.title}”</span>. How did it go?
            </span>
            <StreakChip focus={focus} />
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Button
              type="button"
              size="xs"
              tabIndex={-1}
              onClick={() => {
                onMarkDone(c.taskId);
                focus.answer("done");
              }}
            >
              Done
            </Button>
            <Button
              type="button"
              variant="outline"
              size="xs"
              tabIndex={-1}
              disabled={!canContinue(c.taskId)}
              onClick={() => onMoreTime(c.taskId, c.title)}
            >
              Need more time
            </Button>
            <Button
              type="button"
              variant="outline"
              size="xs"
              tabIndex={-1}
              onClick={() => {
                onBlocked(c.taskId);
                focus.answer("blocked");
              }}
            >
              Blocked
            </Button>
          </div>
        </>
      ) : (
        <>
          <div className="flex items-center gap-2">
            <Coffee className="size-3.5 shrink-0 text-primary" aria-hidden />
            <span className="min-w-0 flex-1 truncate">Nice work. Take a break?</span>
            <StreakChip focus={focus} />
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Button type="button" size="xs" tabIndex={-1} onClick={focus.startBreak}>
              5 minute break
            </Button>
            {canContinue(c.taskId) && (
              <Button
                type="button"
                variant="outline"
                size="xs"
                tabIndex={-1}
                onClick={() => onMoreTime(c.taskId, c.title)}
              >
                Another session
              </Button>
            )}
            <Button type="button" variant="ghost" size="xs" tabIndex={-1} onClick={focus.dismissCheckin}>
              Skip
            </Button>
          </div>
        </>
      )}
      {parked.length > 0 && (
        <>
          <div className="text-muted-foreground">Parked during the session</div>
          <ParkedList items={parked} now={focus.now} onAddTask={onAddTask} onDismiss={focus.unpark} />
        </>
      )}
    </div>
  );
}

function BreakPanel({ focus }: { focus: FocusApi }) {
  const left = Math.max(0, (focus.data.breakEndsAt ?? 0) - focus.now);
  return (
    <div className={`${bar} bg-foreground/[0.03]`} role="region" aria-label="Break">
      <div className="flex items-center gap-2">
        <Coffee className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <span className="min-w-0 flex-1">Break. Step away from the screen.</span>
        <span className="shrink-0 text-base font-semibold tabular-nums" role="timer">
          {formatClock(left)}
        </span>
        <Button type="button" variant="ghost" size="xs" tabIndex={-1} onClick={focus.endBreak}>
          End break
        </Button>
      </div>
    </div>
  );
}

/** No session, but thoughts are waiting: review them. */
function ParkedReview({ focus, onAddTask }: FocusBannerProps) {
  return (
    <div className={bar} role="region" aria-label="Parked thoughts">
      <div className="text-muted-foreground">Parked during focus sessions</div>
      <ParkedList items={focus.data.parked} now={focus.now} onAddTask={onAddTask} onDismiss={focus.unpark} />
    </div>
  );
}

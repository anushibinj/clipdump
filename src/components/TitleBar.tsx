import { memo, type MouseEvent } from "react";
import {
  ClipboardList,
  ListTodo,
  UserCheck,
  NotebookPen,
  Pin,
  PinOff,
  X,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";

export type TitleView = "search" | "notes" | "tasks" | "delegations" | "ask" | "settings";
type TabId = "search" | "notes" | "tasks" | "delegations";

interface TitleBarProps {
  view: TitleView;
  onSwitch: (view: TabId) => void;
  pinned: boolean;
  onTogglePinned: () => void;
  onClose: () => void;
  /** Press on the bar's empty space: hand the mouse to the OS window drag. */
  onDrag: (e: MouseEvent) => void;
  /** Title of the task in flight, shown as a hint on the Tasks icon. */
  workingOn?: string;
  /** Time left in a locked focus session as mm:ss, shown next to the Tasks icon. */
  timeLeft?: string;
  /** Delegations past their deadline: the red circle on the Delegations icon. */
  overdue?: number;
}

const iconBtn =
  "relative flex size-7 shrink-0 items-center justify-center rounded-md transition-colors hover:bg-foreground/[0.06] hover:text-foreground";

/**
 * The window's title bar and its only drag handle: "Clipdump vX.Y.Z" on the left, the view
 * switcher (icons) in the middle, pin and close on the right. Buttons never take focus, so the
 * caret stays in the input below.
 */
export const TitleBar = memo(function TitleBar({
  view,
  onSwitch,
  pinned,
  onTogglePinned,
  onClose,
  onDrag,
  workingOn,
  timeLeft,
  overdue = 0,
}: TitleBarProps) {
  const tabs: {
    id: TabId;
    label: string;
    Icon: LucideIcon;
  }[] = [
    { id: "search", label: "Clipboard (Ctrl+1)", Icon: ClipboardList },
    { id: "notes", label: "Notes (Ctrl+2, Ctrl+Shift+N)", Icon: NotebookPen },
    {
      id: "tasks",
      label: workingOn
        ? `Tasks (Ctrl+3, Ctrl+T), working on: ${workingOn}`
        : "Tasks (Ctrl+3, Ctrl+T)",
      Icon: ListTodo,
    },
    {
      id: "delegations",
      label: overdue > 0 ? `Delegations (Ctrl+4), ${overdue} overdue` : "Delegations (Ctrl+4)",
      Icon: UserCheck,
    },
  ];
  return (
    <div
      onMouseDown={onDrag}
      className="flex h-9 shrink-0 cursor-default select-none items-center gap-2 border-b bg-foreground/[0.03] px-3"
    >
      <span className="text-xs font-medium text-muted-foreground">
        Clipdump v{__APP_VERSION__}
      </span>
      <div className="flex-1" />
      <div
        className="flex items-center gap-0.5"
        role="group"
        aria-label="Switch view"
      >
        {tabs.map(({ id, label, Icon }) => (
          <button
            key={id}
            type="button"
            tabIndex={-1}
            title={label}
            aria-label={label}
            aria-pressed={view === id}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onSwitch(id)}
            className={cn(
              iconBtn,
              id === "tasks" && timeLeft && "w-auto gap-1.5 px-2",
              view === id
                ? "bg-foreground/[0.08] text-foreground"
                : "text-muted-foreground",
            )}
          >
            <Icon className="size-4" aria-hidden />
            {id === "tasks" && timeLeft && (
              <span className="text-xs font-medium tabular-nums" role="timer">
                {timeLeft}
              </span>
            )}
            {id === "delegations" && overdue > 0 && (
              <span
                className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-red-600 px-1 text-[10px] font-semibold leading-none tabular-nums text-white"
                aria-hidden
              >
                {overdue > 99 ? "99+" : overdue}
              </span>
            )}
            {id === "tasks" && workingOn && !timeLeft && (
              <span
                className="absolute right-1 top-1 size-1.5 rounded-full bg-primary"
                aria-hidden
              />
            )}
          </button>
        ))}
      </div>
      <div className="flex items-center gap-0.5 border-l pl-2">
        <button
          type="button"
          tabIndex={-1}
          aria-label={pinned ? "Unpin window" : "Pin window open"}
          aria-pressed={pinned}
          title={
            pinned
              ? "Unpin: hide when focus is lost"
              : "Pin: keep the window open"
          }
          onMouseDown={(e) => e.preventDefault()}
          onClick={onTogglePinned}
          className={cn(
            iconBtn,
            pinned ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {pinned ? (
            <Pin className="size-4 fill-current" aria-hidden />
          ) : (
            <PinOff className="size-4" aria-hidden />
          )}
        </button>
        <button
          type="button"
          tabIndex={-1}
          aria-label="Dismiss"
          title="Dismiss (Esc)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onClose}
          className={cn(iconBtn, "text-muted-foreground")}
        >
          <X className="size-4" aria-hidden />
        </button>
      </div>
    </div>
  );
});

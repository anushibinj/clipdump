import { memo, type MouseEvent } from "react";
import {
  ClipboardList,
  ListTodo,
  NotebookPen,
  Pin,
  PinOff,
  X,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";

export type TitleView = "search" | "notes" | "tasks" | "ask" | "settings";

interface TitleBarProps {
  view: TitleView;
  onSwitch: (view: "search" | "notes" | "tasks") => void;
  pinned: boolean;
  onTogglePinned: () => void;
  onClose: () => void;
  /** Press on the bar's empty space: hand the mouse to the OS window drag. */
  onDrag: (e: MouseEvent) => void;
  /** Title of the task in flight, shown as a hint on the Tasks icon. */
  workingOn?: string;
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
}: TitleBarProps) {
  const tabs: {
    id: "search" | "notes" | "tasks";
    label: string;
    Icon: LucideIcon;
  }[] = [
    { id: "search", label: "Clipboard", Icon: ClipboardList },
    { id: "notes", label: "Notes (Ctrl+Shift+N)", Icon: NotebookPen },
    {
      id: "tasks",
      label: workingOn
        ? `Tasks (Ctrl+T), working on: ${workingOn}`
        : "Tasks (Ctrl+T)",
      Icon: ListTodo,
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
              view === id
                ? "bg-foreground/[0.08] text-foreground"
                : "text-muted-foreground",
            )}
          >
            <Icon className="size-4" aria-hidden />
            {id === "tasks" && workingOn && (
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

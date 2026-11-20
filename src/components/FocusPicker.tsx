import { useState } from "react";
import { Timer } from "lucide-react";
import { Button } from "@/components/ui/button";
import { MAX_MINUTES, MIN_MINUTES, PRESET_MINUTES, clampMinutes } from "@/lib/focus";
import { cn } from "@/lib/utils";

interface FocusPickerProps {
  taskTitle: string;
  /** The length used last, selected first. */
  initialMinutes: number;
  onStart: (minutes: number) => void;
  onCancel: () => void;
}

/**
 * Choose how long to lock in on a task: one click on a preset, or a custom number of minutes.
 * The list is locked the moment Start is pressed, so the picker says so.
 */
export function FocusPicker({ taskTitle, initialMinutes, onStart, onCancel }: FocusPickerProps) {
  const isPreset = (PRESET_MINUTES as readonly number[]).includes(initialMinutes);
  const [minutes, setMinutes] = useState(initialMinutes);
  const [custom, setCustom] = useState(isPreset ? "" : String(initialMinutes));

  const chosen = custom.trim() !== "" ? clampMinutes(Number(custom)) : minutes;
  const customInvalid = custom.trim() !== "" && !/^\d{1,3}$/.test(custom.trim());

  return (
    <div
      role="group"
      aria-label="Focus lock"
      className="flex flex-col gap-2 border-b bg-foreground/[0.03] px-4 py-2.5 text-xs"
      onMouseDown={(e) => {
        // Let the custom field take focus; the chips and buttons keep it in place.
        if ((e.target as HTMLElement).tagName !== "INPUT") e.preventDefault();
      }}
    >
      <div className="flex items-center gap-1.5 font-medium">
        <Timer className="size-3.5" aria-hidden />
        <span className="truncate">Lock in on “{taskTitle}”</span>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {PRESET_MINUTES.map((m) => (
          <button
            key={m}
            type="button"
            tabIndex={-1}
            aria-pressed={custom.trim() === "" && minutes === m}
            onClick={() => {
              setMinutes(m);
              setCustom("");
            }}
            className={cn(
              "rounded-md border px-2 py-1 tabular-nums transition-colors hover:bg-foreground/[0.06]",
              custom.trim() === "" && minutes === m && "border-primary bg-primary/10 font-medium",
            )}
          >
            {m} min
          </button>
        ))}
        <label className="flex items-center gap-1 text-muted-foreground">
          <input
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !customInvalid) {
                e.preventDefault();
                e.stopPropagation();
                onStart(chosen);
              } else if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                onCancel();
              }
            }}
            inputMode="numeric"
            placeholder="custom"
            aria-label={`Custom minutes, ${MIN_MINUTES} to ${MAX_MINUTES}`}
            aria-invalid={customInvalid}
            className="h-7 w-16 rounded-md border bg-transparent px-2 text-foreground tabular-nums outline-none placeholder:text-muted-foreground focus-visible:border-ring"
          />
          min
        </label>
        <div className="ml-auto flex items-center gap-1.5">
          <Button type="button" variant="ghost" size="xs" tabIndex={-1} onClick={onCancel}>
            Cancel
          </Button>
          <Button type="button" size="xs" tabIndex={-1} disabled={customInvalid} onClick={() => onStart(chosen)}>
            Lock for {chosen} min
          </Button>
        </div>
      </div>
      <p className="text-muted-foreground">
        Every other task is locked until the time is up, you finish this one, or you break the lock (30 seconds,
        and your streak starts over).
      </p>
    </div>
  );
}

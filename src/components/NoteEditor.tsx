import { useEffect, useRef, useState, type RefObject } from "react";
import { Pin, PinOff, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { Note, NotePatch } from "@/lib/ipc";
import { MAX_BODY_CHARS, MAX_TAG_CHARS, MAX_TAGS, MAX_TITLE_CHARS, cleanTag } from "@/lib/notes";
import { fullDateTime, relativeTime } from "@/lib/time";
import { cn } from "@/lib/utils";

const SAVE_DELAY_MS = 500;

/** What the editor holds right now, readable from outside (to tell a blank note when leaving it). */
export interface LiveNote {
  id: number;
  title: string;
  body: string;
  tags: string[];
}

interface NoteEditorProps {
  note: Note;
  now: number;
  /** Save a change. The caller reports failures. */
  onSave: (id: number, patch: NotePatch) => void;
  onTogglePin: (id: number) => void;
  onDelete: (id: number) => void;
  /** The delete key was pressed once: the next press deletes. */
  deleteArmed: boolean;
  /** Give focus back to the filter box (Esc inside the editor). */
  onReturnFocus: () => void;
  /** Written on every change, so the view can tell a blank note when leaving it. */
  liveRef: RefObject<LiveNote | null>;
  titleRef: RefObject<HTMLInputElement | null>;
  bodyRef: RefObject<HTMLTextAreaElement | null>;
  tagRef: RefObject<HTMLInputElement | null>;
}

/**
 * One note, beside the list. The title and the body save themselves shortly after you pause (and
 * at once when you pick another note), tags save as you add or remove them, so there is no Save
 * button. The component is keyed by note id: each note starts from its own stored text.
 */
export function NoteEditor({
  note,
  now,
  onSave,
  onTogglePin,
  onDelete,
  deleteArmed,
  onReturnFocus,
  liveRef,
  titleRef,
  bodyRef,
  tagRef,
}: NoteEditorProps) {
  const [title, setTitle] = useState(note.title);
  const [body, setBody] = useState(note.body);
  const [tags, setTags] = useState(note.tags);
  const [tagText, setTagText] = useState("");
  const pending = useRef<NotePatch | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const id = note.id;

  liveRef.current = { id, title, body, tags };

  const flush = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const p = pending.current;
    pending.current = null;
    if (p) saveRef.current(id, p);
  };
  // Leaving this note (another one picked, or the view closed) saves what is waiting.
  useEffect(() => flush, []); // eslint-disable-line react-hooks/exhaustive-deps

  const queue = (patch: NotePatch) => {
    pending.current = { ...pending.current, ...patch };
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(flush, SAVE_DELAY_MS);
  };

  const saveTags = (next: string[]) => {
    setTags(next);
    pending.current = { ...pending.current, tags: next };
    flush();
  };

  const addTag = (raw: string): boolean => {
    const t = cleanTag(raw);
    if (t === "") return false;
    if (tags.includes(t) || tags.length >= MAX_TAGS) return true;
    saveTags([...tags, t]);
    return true;
  };

  const leave = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" || (e.ctrlKey && e.key.toLowerCase() === "e")) {
      e.preventDefault();
      e.stopPropagation();
      flush();
      onReturnFocus();
    }
  };

  return (
    <aside
      aria-label="Note"
      className="flex h-full min-h-0 w-[22rem] shrink-0 flex-col border-l"
      onMouseDown={(e) => e.stopPropagation()}
    >
      <div className="flex h-10 shrink-0 items-center gap-1 border-b pr-2 pl-3 text-xs">
        <span className="truncate text-muted-foreground tabular-nums" title={`Created ${fullDateTime(note.createdAt)}`}>
          Changed {relativeTime(note.updatedAt, now)}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          tabIndex={-1}
          aria-label={note.pinned ? "Unpin note" : "Pin note"}
          aria-pressed={note.pinned}
          title={note.pinned ? "Unpin (Ctrl+P)" : "Pin to the top (Ctrl+P)"}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onTogglePin(id)}
          className={cn("ml-auto", note.pinned ? "text-foreground" : "text-muted-foreground")}
        >
          {note.pinned ? <Pin className="fill-current" aria-hidden /> : <PinOff aria-hidden />}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size={deleteArmed ? "xs" : "icon-xs"}
          tabIndex={-1}
          aria-label="Delete note"
          title="Delete note (Ctrl+Delete, press twice)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onDelete(id)}
          className={deleteArmed ? "text-destructive" : "text-muted-foreground"}
        >
          <Trash2 aria-hidden />
          {deleteArmed && "Delete?"}
        </Button>
      </div>

      <input
        ref={titleRef}
        value={title}
        onChange={(e) => {
          setTitle(e.target.value);
          queue({ title: e.target.value });
        }}
        onBlur={flush}
        onKeyDown={(e) => {
          leave(e);
          // Enter moves on to the body; the title is a single line.
          if (e.key === "Enter" && !e.ctrlKey && !e.shiftKey) {
            e.preventDefault();
            bodyRef.current?.focus();
          }
        }}
        maxLength={MAX_TITLE_CHARS}
        placeholder="Title (optional)"
        aria-label="Note title"
        autoComplete="off"
        spellCheck={false}
        className="mx-3 mt-2 h-8 shrink-0 rounded-md bg-transparent px-1.5 text-sm font-medium outline-none placeholder:font-normal placeholder:text-muted-foreground focus-visible:bg-foreground/[0.04]"
      />

      <textarea
        ref={bodyRef}
        value={body}
        onChange={(e) => {
          setBody(e.target.value);
          queue({ body: e.target.value });
        }}
        onBlur={flush}
        onKeyDown={leave}
        maxLength={MAX_BODY_CHARS}
        placeholder="Write a note"
        aria-label="Note text"
        spellCheck={false}
        className="mx-3 my-1 min-h-0 flex-1 resize-none rounded-md bg-transparent p-1.5 text-sm outline-none placeholder:text-muted-foreground focus-visible:bg-foreground/[0.04]"
      />

      <div className="flex shrink-0 flex-wrap items-center gap-1 border-t px-3 py-2">
        {tags.map((t) => (
          <span
            key={t}
            className="flex items-center gap-0.5 rounded-full bg-foreground/[0.07] py-0.5 pr-1 pl-2 text-xs text-muted-foreground"
          >
            #{t}
            <button
              type="button"
              tabIndex={-1}
              aria-label={`Remove tag ${t}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => saveTags(tags.filter((x) => x !== t))}
              className="rounded-full p-0.5 hover:text-foreground"
            >
              <X className="size-3" aria-hidden />
            </button>
          </span>
        ))}
        <input
          ref={tagRef}
          value={tagText}
          onChange={(e) => {
            const v = e.target.value;
            // A space or comma ends the tag, like Enter.
            if (/[\s,]$/.test(v) && addTag(v)) setTagText("");
            else setTagText(v.replace(/,/g, ""));
          }}
          onBlur={() => {
            if (addTag(tagText)) setTagText("");
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.ctrlKey && !e.shiftKey) {
              e.preventDefault();
              if (addTag(tagText)) setTagText("");
              else bodyRef.current?.focus();
            } else if (e.key === "Backspace" && tagText === "" && tags.length > 0) {
              saveTags(tags.slice(0, -1));
            } else leave(e);
          }}
          maxLength={MAX_TAG_CHARS}
          disabled={tags.length >= MAX_TAGS}
          placeholder={tags.length === 0 ? "Add tags (Ctrl+G)" : "Add tag"}
          aria-label="Add a tag"
          autoComplete="off"
          spellCheck={false}
          className="h-6 min-w-20 flex-1 bg-transparent px-1 text-xs outline-none placeholder:text-muted-foreground"
        />
      </div>
      <div className="px-3 pb-1.5 text-right text-[11px] text-muted-foreground tabular-nums">
        {body.length}/{MAX_BODY_CHARS}
      </div>
    </aside>
  );
}

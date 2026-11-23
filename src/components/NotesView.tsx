import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import { NotebookPen, Plus } from "lucide-react";
import { NoteEditor, type LiveNote } from "@/components/NoteEditor";
import { NoteRow, noteRowId } from "@/components/NoteRow";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import type { NotesApi } from "@/hooks/useNotes";
import { errorMessage, type NotePatch } from "@/lib/ipc";
import { resolveNoteKey } from "@/lib/noteKeys";
import {
  MAX_TITLE_CHARS,
  filterNotes,
  hasTagToken,
  isBlank,
  keepSelection,
  tagCounts,
  toggleTagToken,
} from "@/lib/notes";
import { moveIndex } from "@/lib/shortcuts";
import { cn } from "@/lib/utils";

interface NotesViewProps {
  api: NotesApi;
  /** The App's one input ref: the bar focuses it when it is shown. */
  inputRef: RefObject<HTMLInputElement | null>;
  onNotice: (kind: "error" | "info", message: string) => void;
  /** Hide the bar. */
  onClose: () => void;
}

const LIST_ID = "note-list";
/** How long "press again to delete" stays armed. */
const DELETE_WINDOW_MS = 3000;

/**
 * The notes. The input on top filters (words match the title, the text and the tags; a `#tag`
 * word keeps only notes with that tag), the list shows pinned notes first, and the note picked
 * is edited beside the list. Flat on purpose: no folders, only tags. The keys are in
 * `lib/noteKeys.ts`, what shows and in which order in `lib/notes.ts`.
 */
export function NotesView({
  api,
  inputRef,
  onNotice,
  onClose,
}: NotesViewProps) {
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [armedId, setArmedId] = useState<number | null>(null);
  const armTimer = useRef<number | undefined>(undefined);
  const liveRef = useRef<LiveNote | null>(null);
  const titleRef = useRef<HTMLInputElement | null>(null);
  const bodyRef = useRef<HTMLTextAreaElement | null>(null);
  const tagRef = useRef<HTMLInputElement | null>(null);
  // A new note wants the caret in its body as soon as its editor is on screen.
  const focusNewId = useRef<number | null>(null);

  const rows = useMemo(() => filterNotes(api.notes, query), [api.notes, query]);
  const tags = useMemo(() => tagCounts(api.notes), [api.notes]);
  const selected = keepSelection(rows, selectedId);
  const selectedNote = selected === null ? null : (api.notes.find((n) => n.id === selected) ?? null);

  useEffect(() => {
    if (selected !== selectedId) setSelectedId(selected);
  }, [selected, selectedId]);

  useEffect(() => {
    if (selected !== null) document.getElementById(noteRowId(selected))?.scrollIntoView({ block: "nearest" });
    if (selected !== null && focusNewId.current === selected) {
      focusNewId.current = null;
      bodyRef.current?.focus();
    }
  }, [selected, selectedNote?.id]);

  const latest = useRef({ api, rows, selected, query, armedId });
  latest.current = { api, rows, selected, query, armedId };

  const fail = useCallback((e: unknown) => onNotice("error", errorMessage(e)), [onNotice]);

  const disarm = useCallback(() => {
    window.clearTimeout(armTimer.current);
    setArmedId(null);
  }, []);
  useEffect(() => () => window.clearTimeout(armTimer.current), []);

  // Every change of the picked note goes through here, so a note left blank (a "New note" that
  // got nothing typed in it) is dropped instead of piling up as empty rows.
  const select = useCallback(
    (id: number) => {
      const live = liveRef.current;
      if (live && live.id !== id && isBlank(live) && latest.current.api.notes.some((n) => n.id === live.id)) {
        latest.current.api.remove(live.id).catch(fail);
      }
      setSelectedId(id);
      disarm();
    },
    [disarm, fail],
  );

  const save = useCallback(
    (id: number, patch: NotePatch) => {
      latest.current.api.update(id, patch).catch(fail);
    },
    [fail],
  );

  const togglePin = useCallback(
    (id: number) => {
      const n = latest.current.api.notes.find((x) => x.id === id);
      if (n) save(id, { pinned: !n.pinned });
    },
    [save],
  );

  const create = useCallback(() => {
    latest.current.api.add().then((n) => {
      focusNewId.current = n.id;
      setQuery("");
      select(n.id);
    }, fail);
  }, [fail, select]);

  const remove = useCallback(
    (id: number) => {
      const { rows: shown, api: a, armedId: armed } = latest.current;
      if (armed !== id) {
        // The first press only arms: a note is not something to lose to one stray key.
        setArmedId(id);
        window.clearTimeout(armTimer.current);
        armTimer.current = window.setTimeout(() => setArmedId(null), DELETE_WINDOW_MS);
        onNotice("info", "Press Ctrl+Delete again to delete this note.");
        return;
      }
      disarm();
      const i = shown.findIndex((n) => n.id === id);
      const next = shown[i + 1] ?? shown[i - 1] ?? null;
      // Nothing is left to save on a note that is going away.
      if (liveRef.current?.id === id) liveRef.current = null;
      a.remove(id).then(() => setSelectedId(next?.id ?? null), fail);
    },
    [disarm, fail, onNotice],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      const action = resolveNoteKey(e.nativeEvent);
      if (!action) return;
      const { rows: shown, selected: sel, query: q } = latest.current;
      switch (action.type) {
        case "new":
          break;
        case "move": {
          if (shown.length === 0) return;
          const at = shown.findIndex((n) => n.id === sel);
          select(shown[moveIndex(at, action.delta, shown.length)].id);
          break;
        }
        case "togglePin":
          if (sel !== null) togglePin(sel);
          break;
        case "delete":
          if (sel !== null) remove(sel);
          break;
        case "editBody":
          if (sel === null) return; // Tab with nothing to edit: leave it to the browser
          bodyRef.current?.focus();
          break;
        case "editTags":
          if (sel === null) return;
          tagRef.current?.focus();
          break;
        case "escape":
          if (q !== "") setQuery("");
          else onClose();
          break;
      }
      e.preventDefault();
      if (action.type === "new") create();
    },
    [create, onClose, remove, select, togglePin],
  );

  const returnFocus = useCallback(() => inputRef.current?.focus(), [inputRef]);

  const count = api.notes.length;
  const filtering = query.trim() !== "";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div>
        <div className="flex h-12 items-center gap-2.5 border-b px-4 transition-shadow has-[input:focus-visible]:border-ring has-[input:focus-visible]:shadow-[inset_0_-1px_0_0_var(--ring)]">
          <NotebookPen className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Filter notes. Use #tag to filter by tag"
            role="combobox"
            aria-expanded="true"
            aria-controls={LIST_ID}
            aria-autocomplete="none"
            aria-label="Filter notes"
            aria-activedescendant={selected === null ? undefined : noteRowId(selected)}
            maxLength={MAX_TITLE_CHARS}
            autoFocus
            autoComplete="off"
            spellCheck={false}
            className="h-11 min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
          />
          <div className="flex shrink-0 items-center gap-0.5">
            <Button
              type="button"
              variant="ghost"
              size="xs"
              tabIndex={-1}
              title="New note (Ctrl+Enter)"
              onMouseDown={(e) => e.preventDefault()}
              onClick={create}
              className="gap-1.5"
            >
              <Plus aria-hidden />
              New
              <Kbd className="h-4 min-w-4 px-1 text-[10px]">Ctrl ↵</Kbd>
            </Button>
          </div>
        </div>

        <div className="flex h-10 items-center gap-3 border-b px-4 text-xs text-muted-foreground">
          <span className="shrink-0 tabular-nums">
            {filtering ? `${rows.length} of ${count}` : `${count} ${count === 1 ? "note" : "notes"}`}
          </span>
          {tags.length > 0 && (
            <div
              className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
              role="group"
              aria-label="Tags"
              onMouseDown={(e) => {
                // Clicks on a chip keep the caret in the filter box.
                e.stopPropagation();
                e.preventDefault();
              }}
            >
              {tags.map(({ tag, count: c }) => {
                const on = hasTagToken(query, tag);
                return (
                  <button
                    key={tag}
                    type="button"
                    tabIndex={-1}
                    aria-pressed={on}
                    title={on ? `Stop filtering by #${tag}` : `Only notes tagged #${tag}`}
                    onClick={() => setQuery(toggleTagToken(query, tag))}
                    className={cn(
                      "shrink-0 rounded-full px-2 py-0.5 transition-colors",
                      on
                        ? "bg-primary text-primary-foreground"
                        : "bg-foreground/[0.06] hover:bg-foreground/[0.1] hover:text-foreground",
                    )}
                  >
                    #{tag} <span className="tabular-nums opacity-70">{c}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        <div
          id={LIST_ID}
          role="listbox"
          aria-label="Notes"
          className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain py-1"
        >
          {rows.length === 0 ? (
            <Empty ready={api.ready} error={api.error} total={count} onNew={create} />
          ) : (
            rows.map((n) => (
              <NoteRow
                key={n.id}
                note={n}
                selected={n.id === selected}
                now={api.now}
                onSelect={select}
                onTogglePin={togglePin}
              />
            ))
          )}
        </div>
        {selectedNote && (
          <NoteEditor
            key={selectedNote.id}
            note={selectedNote}
            now={api.now}
            onSave={save}
            onTogglePin={togglePin}
            onDelete={remove}
            deleteArmed={armedId === selectedNote.id}
            onReturnFocus={returnFocus}
            liveRef={liveRef}
            titleRef={titleRef}
            bodyRef={bodyRef}
            tagRef={tagRef}
          />
        )}
      </div>
    </div>
  );
}

function Empty({
  ready,
  error,
  total,
  onNew,
}: {
  ready: boolean;
  error: string | null;
  total: number;
  onNew: () => void;
}) {
  if (!ready) return null;
  let title: string;
  let hint: string;
  if (error) {
    title = "Notes are not available";
    hint = error;
  } else if (total === 0) {
    title = "No notes yet";
    hint = "Press Ctrl+Enter to write the first one. Titles and tags are optional.";
  } else {
    title = "No match";
    hint = "No note has those words or tags. Clear the filter with Esc.";
  }
  return (
    <div className="flex h-full min-h-40 flex-col items-center justify-center gap-1.5 px-6 text-center">
      <NotebookPen className="mb-1 size-8 text-muted-foreground/60" strokeWidth={1.5} aria-hidden />
      <p className="text-sm font-medium">{title}</p>
      <p className="max-w-80 text-xs text-muted-foreground">{hint}</p>
      {!error && total === 0 && (
        <Button type="button" variant="outline" size="xs" tabIndex={-1} onClick={onNew} className="mt-1">
          <Plus aria-hidden />
          New note
        </Button>
      )}
    </div>
  );
}

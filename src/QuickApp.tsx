import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { File, FileText, Folder, Image as ImageIcon, Pin, PinOff } from "lucide-react";
import { Highlighted } from "@/components/Highlighted";
import {
  hideQuick,
  onHistoryChanged,
  onQuickShown,
  quickCopy,
  search,
  setQuickPinned,
  type ClipKind,
  type ResultDto,
} from "@/lib/ipc";
import { QUICK_LIMIT, QUICK_VISIBLE, rowForDigit, stepSelection } from "@/lib/quick";
import { cn } from "@/lib/utils";

const KIND_ICON: Record<ClipKind, typeof FileText> = {
  text: FileText,
  image: ImageIcon,
  files: File,
};

/**
 * The tiny clipboard (Ctrl twice): one input, the recent history (pinned items in their natural place), three rows visible and the rest scrolls, Enter or a click copies.
 * Up/Down move, Alt+1..3 copy a row directly, Esc closes. The pin keeps it open after a copy
 * and when the focus goes elsewhere.
 */
export default function QuickApp() {
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<ResultDto[]>([]);
  const [sel, setSel] = useState(0);
  const [pinned, setPinned] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const queryRef = useRef(query);
  queryRef.current = query;
  // Drops the answer of an older search that finishes after a newer one.
  const seq = useRef(0);

  const run = useCallback((q: string) => {
    const mine = ++seq.current;
    search(q, "all", QUICK_LIMIT, false)
      .then((r) => {
        if (mine !== seq.current) return;
        setRows(r.slice(0, QUICK_LIMIT));
        setSel(0);
      })
      .catch(() => {
        if (mine === seq.current) setRows([]);
      });
  }, []);

  useEffect(() => run(query), [query, run]);

  useEffect(() => {
    const subs: Promise<UnlistenFn>[] = [
      onQuickShown(() => {
        setQuery("");
        setError(null);
        setPinned(false);
        run("");
        input.current?.focus();
      }),
      onHistoryChanged(() => run(queryRef.current)),
    ];
    return () => {
      subs.forEach((p) => void p.then((un) => un()));
    };
  }, [run]);

  // Keep the selected row in view while stepping with the arrow keys.
  useEffect(() => {
    list.current?.children[sel]?.scrollIntoView({ block: "nearest" });
  }, [sel, rows]);

  const copy = useCallback((r: ResultDto | undefined) => {
    if (!r) return;
    quickCopy(r.id).catch((e) => setError(typeof e === "string" ? e : "could not copy"));
  }, []);

  const togglePinned = () => {
    const next = !pinned;
    setPinned(next);
    void setQuickPinned(next);
    input.current?.focus();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      void hideQuick();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => stepSelection(s, 1, rows.length));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => stepSelection(s, -1, rows.length));
    } else if (e.key === "Enter") {
      e.preventDefault();
      copy(rows[sel]);
    } else if (e.altKey) {
      const i = rowForDigit(e.key, rows.length);
      if (i !== null) {
        e.preventDefault();
        copy(rows[i]);
      }
    }
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden rounded-lg border border-border bg-popover text-popover-foreground">
      <div className="flex items-center gap-1 border-b border-border px-2">
        <input
          ref={input}
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Search clipboard"
          spellCheck={false}
          autoComplete="off"
          className="h-9 min-w-0 flex-1 bg-transparent px-1 text-sm outline-none placeholder:text-muted-foreground"
        />
        <button
          type="button"
          tabIndex={-1}
          onMouseDown={(e) => e.preventDefault()}
          onClick={togglePinned}
          aria-label={pinned ? "Unpin" : "Pin open"}
          aria-pressed={pinned}
          className={cn(
            "rounded p-1 text-muted-foreground hover:bg-accent hover:text-accent-foreground",
            pinned && "text-foreground",
          )}
        >
          {pinned ? <PinOff className="size-3.5" /> : <Pin className="size-3.5" />}
        </button>
      </div>
      <ul ref={list} role="listbox" className="flex-1 overflow-y-auto p-1">
        {rows.map((r, i) => {
          const Icon = r.kind === "files" && r.fileCount === 1 ? Folder : KIND_ICON[r.kind];
          const text = r.snippet ?? r.preview;
          return (
            <li
              key={r.id}
              role="option"
              aria-selected={i === sel}
              onMouseEnter={() => setSel(i)}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => copy(r)}
              className={cn(
                "flex h-[42px] cursor-default items-center gap-2 rounded px-2 text-sm",
                i === sel && "bg-accent text-accent-foreground",
              )}
            >
              <Icon className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="line-clamp-2 min-w-0 flex-1 break-words text-xs leading-snug">
                <Highlighted text={text} indices={r.matchIndices} />
              </span>
              {i < QUICK_VISIBLE && (
                <kbd className="shrink-0 text-[10px] text-muted-foreground">Alt+{i + 1}</kbd>
              )}
            </li>
          );
        })}
        {rows.length === 0 && (
          <li className="px-2 py-3 text-xs text-muted-foreground">
            {query ? "No matches" : "Nothing copied yet"}
          </li>
        )}
      </ul>
      {error && <p className="px-3 pb-1 text-[11px] text-destructive">{error}</p>}
    </div>
  );
}

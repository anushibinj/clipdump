import { memo } from "react";
import { File, FileText, Folder, Image as ImageIcon, Pin } from "lucide-react";
import { Highlighted } from "@/components/Highlighted";
import { Badge } from "@/components/ui/badge";
import { Kbd } from "@/components/ui/kbd";
import { formatBytes } from "@/lib/bytes";
import { thumbUrl, type ResultDto } from "@/lib/ipc";
import { fullTime, relativeTime } from "@/lib/time";
import { cn } from "@/lib/utils";

/** DOM id of a row, shared with `aria-activedescendant` and scroll-into-view. */
export const rowId = (id: number) => `clip-row-${id}`;

interface ResultItemProps {
  result: ResultDto;
  /** Position in the list; the first nine show their Ctrl+N hint. */
  index: number;
  selected: boolean;
  /** Minute rounded "now" (lib/time.ts `minuteNow`), so the memo holds within a minute. */
  now: number;
  onActivate: (id: number) => void;
}

/** Same row, same props: skip the render. The result objects are new on every response. */
export function sameRow(a: ResultItemProps, b: ResultItemProps): boolean {
  if (
    a.index !== b.index ||
    a.selected !== b.selected ||
    a.now !== b.now ||
    a.onActivate !== b.onActivate
  ) {
    return false;
  }
  const x = a.result;
  const y = b.result;
  if (x === y) return true;
  if (
    x.id !== y.id ||
    x.kind !== y.kind ||
    x.preview !== y.preview ||
    x.snippet !== y.snippet ||
    x.createdAt !== y.createdAt ||
    x.pinned !== y.pinned ||
    x.hasThumb !== y.hasThumb ||
    x.byteSize !== y.byteSize ||
    x.fileCount !== y.fileCount ||
    x.missingFiles !== y.missingFiles ||
    x.lineCount !== y.lineCount ||
    x.width !== y.width ||
    x.height !== y.height ||
    x.matchIndices.length !== y.matchIndices.length
  ) {
    return false;
  }
  for (let i = 0; i < x.matchIndices.length; i += 1) {
    if (x.matchIndices[i] !== y.matchIndices[i]) return false;
  }
  return true;
}

function Thumb({ r }: { r: ResultDto }) {
  return (
    <img
      // `?v=createdAt` keeps a reused id from showing an old cached picture.
      src={thumbUrl(r.id, r.createdAt)}
      alt=""
      loading="lazy"
      decoding="async"
      draggable={false}
      className="size-10 shrink-0 rounded-md border bg-muted object-cover"
    />
  );
}

function missingLabel(r: ResultDto): string {
  return r.missingFiles >= r.fileCount ? "missing" : `${r.missingFiles} missing`;
}

/** First name of a file preview ("a.txt, b.png, c" is the names joined by ", "). */
function firstName(preview: string): string {
  const at = preview.indexOf(", ");
  return at < 0 ? preview : preview.slice(0, at);
}

function Body({ r }: { r: ResultDto }) {
  const matched = r.matchIndices.length > 0;
  // Highlight positions index the snippet when there is one, else the preview.
  const shown = r.snippet ?? r.preview;

  if (r.kind === "image") {
    return (
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-sm font-medium">
          <span className="truncate">
            {r.width}x{r.height}, {formatBytes(r.byteSize)}
          </span>
          {r.snippet !== null && (
            <Badge variant="outline" className="h-4 px-1.5 text-[10px] font-semibold">
              OCR
            </Badge>
          )}
        </div>
        {r.snippet !== null ? (
          <div className="truncate text-xs text-muted-foreground">
            <Highlighted text={r.snippet} indices={r.matchIndices} />
          </div>
        ) : (
          <div className="truncate text-xs text-muted-foreground">Screenshot or image</div>
        )}
      </div>
    );
  }

  if (r.kind === "files") {
    const useName = !matched && r.snippet === null;
    const more = r.fileCount - 1;
    return (
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate">
            {useName ? firstName(r.preview) : <Highlighted text={shown} indices={r.matchIndices} />}
          </span>
          {useName && more > 0 && (
            <span className="shrink-0 text-xs text-muted-foreground">+{more} more</span>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="min-w-0 flex-1 truncate">
      <Highlighted text={shown} indices={r.matchIndices} />
    </div>
  );
}

function Leading({ r }: { r: ResultDto }) {
  if (r.kind === "image") {
    return r.hasThumb ? (
      <Thumb r={r} />
    ) : (
      <span className="flex size-10 shrink-0 items-center justify-center rounded-md border bg-muted text-muted-foreground">
        <ImageIcon className="size-4" />
      </span>
    );
  }
  if (r.kind === "files") {
    if (r.hasThumb) return <Thumb r={r} />;
    const Icon = r.fileCount === 1 && r.preview.indexOf(".") < 0 ? Folder : File;
    return <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />;
  }
  return <FileText className="size-4 shrink-0 text-muted-foreground" aria-hidden />;
}

/**
 * One history row. Memoized with `sameRow`, so a keystroke repaints only the
 * rows whose content changed, and moving the selection repaints two rows.
 *
 * Not a cmdk `CommandItem`: that registers every row in cmdk's store and
 * owns the selection, which fights the explicit selection the keyboard
 * layer needs (selection survives refreshes, follows a pinned item, picks the
 * neighbor after a delete). The row keeps the ARIA contract instead: the list
 * is cmdk's `role="listbox"`, rows are `role="option"` with `aria-selected`, and the
 * input points at the active row with `aria-activedescendant`.
 */
export const ResultItem = memo(function ResultItem({
  result: r,
  index,
  selected,
  now,
  onActivate,
}: ResultItemProps) {
  const tall = r.kind === "image" || (r.kind === "files" && r.hasThumb);
  return (
    <div
      id={rowId(r.id)}
      role="option"
      aria-selected={selected}
      data-selected={selected}
      data-id={r.id}
      data-kind={r.kind}
      className={cn(
        "group/row mx-1 flex cursor-default items-center gap-3 rounded-lg px-2.5 text-sm transition-colors",
        // Clipboard text is shown literally: no ligatures (Geist turns -> into an arrow).
        "scroll-my-1 [font-variant-ligatures:none] data-[selected=true]:bg-foreground/[0.07] dark:data-[selected=true]:bg-foreground/[0.1]",
        // Rows below the fold skip layout and paint until scrolled near (about 50 rows are rendered, 8 are seen).
        "[content-visibility:auto] [contain-intrinsic-size:auto_44px]",
        tall ? "min-h-[52px] py-1.5" : "min-h-10 py-1.5",
      )}
      // Keep keyboard focus in the search input.
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => onActivate(r.id)}
    >
      <Leading r={r} />
      <Body r={r} />
      <div className="flex shrink-0 items-center gap-2">
        {r.kind === "text" && r.lineCount > 1 && (
          <Badge variant="secondary" className="h-4 px-1.5 text-[10px] font-medium tabular-nums">
            {r.lineCount} lines
          </Badge>
        )}
        {r.kind === "files" && r.fileCount > 1 && r.missingFiles === 0 && (r.matchIndices.length > 0 || r.snippet !== null) && (
          <Badge variant="secondary" className="h-4 px-1.5 text-[10px] font-medium tabular-nums">
            {r.fileCount} files
          </Badge>
        )}
        {r.missingFiles > 0 && (
          <Badge
            variant="outline"
            className="h-4 border-destructive/30 bg-destructive/10 px-1.5 text-[10px] font-medium text-red-700 dark:text-destructive"
          >
            {missingLabel(r)}
          </Badge>
        )}
        <span
          className="w-7 text-right text-xs text-muted-foreground tabular-nums"
          title={fullTime(r.createdAt)}
        >
          {relativeTime(r.createdAt, now)}
        </span>
        <span className="flex w-3.5 justify-center" aria-label={r.pinned ? "Pinned" : undefined}>
          {r.pinned && <Pin className="size-3.5 fill-current text-muted-foreground" aria-hidden />}
        </span>
        <span className="hidden w-[46px] justify-end @min-[460px]:flex">
          {index < 9 && (
            <Kbd className="group-data-[selected=true]/row:bg-foreground/10 group-data-[selected=true]/row:text-foreground/80">Ctrl {index + 1}</Kbd>
          )}
        </span>
      </div>
    </div>
  );
}, sameRow);

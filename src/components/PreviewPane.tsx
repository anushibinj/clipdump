import { memo, type ReactNode } from "react";
import { FileText, Files, Image as ImageIcon, Sparkles } from "lucide-react";
import { FileList } from "@/components/FileList";
import { ImagePreview } from "@/components/ImagePreview";
import { SplitTab } from "@/components/SplitTab";
import type { SplitControl } from "@/hooks/useSplit";
import type { RightTab } from "@/lib/pane";
import { ScrollArea } from "@/components/ui/scroll-area";
import { formatBytes } from "@/lib/bytes";
import type { PreviewDto, ResultDto } from "@/lib/ipc";
import { cn } from "@/lib/utils";

interface PreviewPaneProps {
  /** The selected row (for the cache buster of the picture URLs), or null. */
  result: ResultDto | null;
  preview: PreviewDto | null;
  error: string | null;
  ocrHint: string;
  tab: RightTab;
  onTabChange: (tab: RightTab) => void;
  /** The AI split of the selected item (shown on the split tab). */
  split: SplitControl;
  /** A footer message for the bar. */
  onError: (message: string) => void;
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      tabIndex={-1}
      // Keep the caret in the search input.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={cn(
        "flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50",
        active ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function Header({ icon: Icon, title, meta }: { icon: typeof FileText; title: string; meta: string }) {
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs">
      <Icon className="size-3.5 text-muted-foreground" aria-hidden />
      <span className="font-medium">{title}</span>
      <span className="ml-auto text-muted-foreground tabular-nums">{meta}</span>
    </div>
  );
}

/**
 * The pane beside the list (Tab): the full text in monospace, the image with
 * its OCR text, or the file list. Shows the previous item dimmed while the next
 * one loads, so arrowing through items does not flash empty.
 */
export const PreviewPane = memo(function PreviewPane({
  result,
  preview,
  error,
  ocrHint,
  tab,
  onTabChange,
  split,
  onError,
}: PreviewPaneProps) {
  const stale = preview !== null && result !== null && preview.id !== result.id;
  return (
    <aside
      aria-label="Preview"
      className="flex h-2/5 min-h-0 w-full min-w-0 shrink-0 flex-col border-t bg-muted/20 @xl/body:h-auto @xl/body:w-[46%] @xl/body:border-t-0 @xl/body:border-l"
    >
      <div role="tablist" aria-label="Right pane" className="flex h-9 shrink-0 items-center gap-1 border-b px-2">
        <TabButton active={tab === "preview"} onClick={() => onTabChange("preview")}>
          <FileText className="size-3.5" aria-hidden />
          Preview
        </TabButton>
        <TabButton active={tab === "split"} onClick={() => onTabChange("split")}>
          <Sparkles className="size-3.5" aria-hidden />
          AI split
        </TabButton>
      </div>
      {tab === "split" ? (
        <SplitTab kind={result?.kind ?? null} split={split} onError={onError} />
      ) : error ? (
        <p role="alert" className="p-4 text-xs text-muted-foreground">
          The preview could not be loaded: {error}
        </p>
      ) : preview === null ? (
        <p className="p-4 text-xs text-muted-foreground">
          {result === null ? "Nothing selected." : "Loading..."}
        </p>
      ) : (
        <div
          className={cn("flex min-h-0 flex-1 flex-col transition-opacity duration-100", stale && "opacity-50")}
        >
          {preview.kind === "text" && (
            <>
              <Header
                icon={FileText}
                title="Text"
                meta={`${preview.totalChars.toLocaleString()} chars, ${formatBytes(preview.byteSize)}`}
              />
              <ScrollArea className="min-h-0 flex-1">
                <pre className="p-3 font-mono text-xs/5 whitespace-pre-wrap break-words select-text">
                  {preview.text}
                </pre>
                {preview.truncated && (
                  <p className="border-t px-3 py-2 text-xs text-muted-foreground">
                    Only the first part is shown. The whole text is copied.
                  </p>
                )}
              </ScrollArea>
            </>
          )}
          {preview.kind === "image" && (
            <>
              <Header icon={ImageIcon} title="Image" meta={`${preview.width}x${preview.height}`} />
              <ImagePreview
                key={preview.id}
                id={preview.id}
                createdAt={result?.createdAt ?? 0}
                width={preview.width}
                height={preview.height}
                byteSize={preview.byteSize}
                ocrText={preview.ocrText}
                ocrTruncated={preview.ocrTruncated}
                ocrStatus={preview.ocrStatus}
                ocrHint={ocrHint}
              />
            </>
          )}
          {preview.kind === "files" && (
            <>
              <Header
                icon={Files}
                title="Files"
                meta={`${preview.totalFiles} ${preview.totalFiles === 1 ? "item" : "items"}`}
              />
              <FileList
                files={preview.files}
                totalFiles={preview.totalFiles}
                truncated={preview.truncated}
                totalSize={preview.totalSize}
              />
            </>
          )}
        </div>
      )}
    </aside>
  );
});

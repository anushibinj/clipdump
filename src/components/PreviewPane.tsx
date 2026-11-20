import { memo, type ReactNode } from "react";
import { FileText, Files, Image as ImageIcon, Loader2, Sparkles } from "lucide-react";
import { FileList } from "@/components/FileList";
import { ImagePreview } from "@/components/ImagePreview";
import { SplitParts } from "@/components/SplitParts";
import { Button } from "@/components/ui/button";
import type { SplitControl } from "@/hooks/useSplit";
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
  /** Split state of the selected item (text items only). */
  split: SplitControl;
  /** A footer message for the bar. */
  onError: (message: string) => void;
}

function Header({
  icon: Icon,
  title,
  meta,
  action,
}: {
  icon: typeof FileText;
  title: string;
  meta: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs">
      <Icon className="size-3.5 text-muted-foreground" aria-hidden />
      <span className="font-medium">{title}</span>
      <span className="ml-auto text-muted-foreground tabular-nums">{meta}</span>
      {action}
    </div>
  );
}

/** The Split button of a text preview: asks the LLM, or goes back to the plain text. */
function SplitButton({ split }: { split: SplitControl }) {
  const { status } = split.state;
  if (status === "done") {
    return (
      <Button type="button" variant="ghost" size="xs" tabIndex={-1} onClick={split.reset} onMouseDown={(e) => e.preventDefault()}>
        Show text
      </Button>
    );
  }
  return (
    <Button
      type="button"
      variant="outline"
      size="xs"
      tabIndex={-1}
      disabled={status === "loading"}
      title="Split into copyable items with your LLM (Ctrl+Shift+S)"
      onClick={split.run}
      onMouseDown={(e) => e.preventDefault()}
    >
      {status === "loading" ? <Loader2 className="animate-spin" aria-hidden /> : <Sparkles aria-hidden />}
      {status === "loading" ? "Splitting..." : "Split"}
    </Button>
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
  split,
  onError,
}: PreviewPaneProps) {
  const stale = preview !== null && result !== null && preview.id !== result.id;
  return (
    <aside
      aria-label="Preview"
      className="flex w-[46%] min-w-0 shrink-0 flex-col border-l bg-muted/20"
    >
      {error ? (
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
                action={stale ? null : <SplitButton split={split} />}
              />
              {split.state.status === "error" && (
                <p role="alert" className="border-b bg-destructive/10 px-3 py-1.5 text-xs text-red-700 dark:text-destructive">
                  {split.state.message}
                </p>
              )}
              {split.state.status === "done" && !stale ? (
                <SplitParts parts={split.state.parts} onError={onError} />
              ) : (
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
              )}
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

import { useState } from "react";
import { Loader2 } from "lucide-react";
import { formatBytes } from "@/lib/bytes";
import { blobUrl, thumbUrl, type OcrStatus } from "@/lib/ipc";

interface ImagePreviewProps {
  id: number;
  /** Cache buster for the clip URLs (see ipc.ts `thumbUrl`). */
  createdAt: number;
  width: number;
  height: number;
  byteSize: number;
  ocrText: string;
  ocrTruncated: boolean;
  ocrStatus: OcrStatus;
  /** Only shown when OCR cannot run (`StatsDto.ocrHint`). */
  ocrHint: string;
}

function OcrNote({ status, hint, empty }: { status: OcrStatus; hint: string; empty: boolean }) {
  if (status === "pending") {
    return (
      <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Loader2 className="size-3 animate-spin" aria-hidden /> Recognizing text...
      </p>
    );
  }
  if (status === "unavailable") {
    return (
      <p className="text-xs text-muted-foreground">
        Text recognition is not available. {hint}
      </p>
    );
  }
  if (status === "failed") {
    return <p className="text-xs text-muted-foreground">Text recognition failed for this image.</p>;
  }
  if (empty) return <p className="text-xs text-muted-foreground">No text found in this image.</p>;
  return null;
}

/** The Tab pane for an image: the picture fitted to the pane, the OCR text below. */
export function ImagePreview({
  id,
  createdAt,
  width,
  height,
  byteSize,
  ocrText,
  ocrTruncated,
  ocrStatus,
  ocrHint,
}: ImagePreviewProps) {
  const [loaded, setLoaded] = useState(false);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div
        className="relative flex h-[48%] min-h-24 shrink-0 items-center justify-center overflow-hidden border-b bg-muted/40 p-2"
        // The thumbnail is already in RAM: it shows at once and the full PNG fades in over it.
        style={{
          backgroundImage: `url(${thumbUrl(id, createdAt)})`,
          backgroundSize: "contain",
          backgroundPosition: "center",
          backgroundRepeat: "no-repeat",
        }}
      >
        <img
          key={id}
          src={blobUrl(id, createdAt)}
          alt={`Image ${width} by ${height}`}
          onLoad={() => setLoaded(true)}
          draggable={false}
          className="max-h-full max-w-full rounded-sm object-contain opacity-0 shadow-sm transition-opacity duration-150 data-[loaded=true]:opacity-100"
          data-loaded={loaded}
        />
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-1.5 px-3 py-2">
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span className="font-medium text-foreground">OCR text</span>
          <span className="tabular-nums">
            {width}x{height}, {formatBytes(byteSize)}
          </span>
        </div>
        {ocrText !== "" && (
          <pre className="min-h-0 flex-1 overflow-y-auto font-mono text-xs/5 whitespace-pre-wrap break-words select-text">
            {ocrText}
            {ocrTruncated && "\n..."}
          </pre>
        )}
        <OcrNote status={ocrStatus} hint={ocrHint} empty={ocrText === "" && ocrStatus === "done"} />
      </div>
    </div>
  );
}

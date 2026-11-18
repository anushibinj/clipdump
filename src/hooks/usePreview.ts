import { useEffect, useRef, useState } from "react";
import { errorMessage, getPreview, type PreviewDto } from "@/lib/ipc";
import { createLatest } from "@/lib/latest";

export interface PreviewState {
  /** The newest preview received. Its `id` may lag `wantedId` while a request is in flight. */
  preview: PreviewDto | null;
  error: string | null;
}

/**
 * Loads the Tab pane content for the selected item while the pane is open.
 * Same rule as search: a response is applied only if it is the newest request,
 * so holding an arrow key never paints an old item last. `refreshKey` re-reads
 * the same item (the OCR text arrives after the picture, see `history:changed`).
 */
export function usePreview(wantedId: number | null, enabled: boolean, refreshKey: number): PreviewState {
  const [state, setState] = useState<PreviewState>({ preview: null, error: null });
  const latest = useRef(createLatest());

  useEffect(() => {
    if (!enabled || wantedId === null) return;
    const ticket = latest.current.next();
    getPreview(wantedId).then(
      (preview) => {
        if (latest.current.isCurrent(ticket)) setState({ preview, error: null });
      },
      (e: unknown) => {
        if (latest.current.isCurrent(ticket)) setState({ preview: null, error: errorMessage(e) });
      },
    );
  }, [wantedId, enabled, refreshKey]);

  return state;
}

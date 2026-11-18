import { memo, useMemo } from "react";
import { highlightSegments, windowAroundFirstHit } from "@/lib/highlight";

interface HighlightedProps {
  text: string;
  /** Character positions into `text` (see lib/highlight.ts). */
  indices: readonly number[];
  /** Start the line near the first match when it would be cut off by the row width. */
  windowed?: boolean;
}

/** One line of text with the matched characters marked. Plain text when there is nothing to mark. */
export const Highlighted = memo(function Highlighted({
  text,
  indices,
  windowed = true,
}: HighlightedProps) {
  const segments = useMemo(() => {
    if (indices.length === 0) return null;
    const w = windowed ? windowAroundFirstHit(text, indices) : { text, indices: indices as number[] };
    return highlightSegments(w.text, w.indices);
  }, [text, indices, windowed]);
  if (!segments) return <>{text}</>;
  return (
    <>
      {segments.map((s, i) =>
        s.hit ? (
          <mark
            key={i}
            className="rounded-[3px] bg-amber-400/35 px-px text-foreground dark:bg-amber-400/25"
          >
            {s.text}
          </mark>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
    </>
  );
});

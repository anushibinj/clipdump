// Match highlighting. The backend sends sorted CHARACTER positions (Unicode
// code points, not UTF-16 units and not bytes) into the string the row shows:
// the snippet when there is one, else the preview. JavaScript strings index by
// UTF-16 unit, so an emoji or other astral character is two units but one
// position. This walks code points to keep both in step.

export interface Segment {
  text: string;
  hit: boolean;
}

/** Split `text` into alternating plain and highlighted runs. */
export function highlightSegments(text: string, indices: readonly number[]): Segment[] {
  if (indices.length === 0 || text.length === 0) return [{ text, hit: false }];
  const out: Segment[] = [];
  let runStart = 0; // UTF-16 offset where the current run began
  let runHit = false;
  let unit = 0; // UTF-16 offset of the current code point
  let pos = 0; // code point position
  let next = 0; // index into `indices`
  for (const ch of text) {
    while (next < indices.length && indices[next] < pos) next += 1;
    const hit = next < indices.length && indices[next] === pos;
    if (hit !== runHit && unit > runStart) {
      out.push({ text: text.slice(runStart, unit), hit: runHit });
      runStart = unit;
    }
    runHit = hit;
    unit += ch.length;
    pos += 1;
  }
  if (unit > runStart) out.push({ text: text.slice(runStart, unit), hit: runHit });
  return out;
}

export interface Windowed {
  text: string;
  indices: number[];
}

/**
 * The row is one truncated line, so a match far to the right of the preview
 * would be cut off by CSS. When the first match is further than `maxLead`
 * characters in, start the text a little before it (with a leading ellipsis)
 * and shift the positions to match. Pure and cheap: only runs for rows that
 * have highlights.
 */
export function windowAroundFirstHit(
  text: string,
  indices: readonly number[],
  maxLead = 36,
  keepLead = 12,
): Windowed {
  if (indices.length === 0 || indices[0] <= maxLead) {
    return { text, indices: indices.slice() };
  }
  const startPos = indices[0] - keepLead;
  // Convert the start position (code points) to a UTF-16 offset.
  let unit = 0;
  let pos = 0;
  for (const ch of text) {
    if (pos === startPos) break;
    unit += ch.length;
    pos += 1;
  }
  const prefix = "…"; // ellipsis, one character
  return {
    text: prefix + text.slice(unit),
    // The ellipsis takes position 0, the old `startPos` becomes 1.
    indices: indices.filter((i) => i >= startPos).map((i) => i - startPos + 1),
  };
}

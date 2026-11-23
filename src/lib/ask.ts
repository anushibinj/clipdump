// Pure helpers of the Ask view: turning an answer into text and citation chips, finding the verified
// quote inside an excerpt, and the words that say what was searched.

import type { AskAnswer, AskHit, AskSearched, AskSource, AskTurn } from "@/lib/ipc";

/** Finished questions kept for a conversation, and sent along with a follow-up (the backend uses the last three). */
export const MAX_HISTORY = 6;

/** One finished question with the answer it got. */
export interface AskExchange {
  question: string;
  answer: AskAnswer;
}

/** What is sent as history: the answer text only when one was found. */
export function toHistory(turns: readonly AskExchange[]): AskTurn[] {
  return turns.slice(-MAX_HISTORY).map((t) => ({
    question: t.question,
    answer: t.answer.found ? t.answer.answer : "",
  }));
}

export type AnswerPart = { kind: "text"; text: string } | { kind: "cite"; n: number };

/**
 * The answer as text and `[n]` citation markers. A marker is `[` digits `]`; anything else in square
 * brackets stays text. Markers that name no hit are kept as text too, so nothing is silently lost
 * (the backend already removed the ones that did not verify).
 */
export function splitAnswer(answer: string, hits: readonly Pick<AskHit, "n">[]): AnswerPart[] {
  const known = new Set(hits.map((h) => h.n));
  const parts: AnswerPart[] = [];
  let text = "";
  const flush = () => {
    if (text !== "") parts.push({ kind: "text", text });
    text = "";
  };
  const re = /\[(\d{1,3})\]/g;
  let last = 0;
  for (let m = re.exec(answer); m !== null; m = re.exec(answer)) {
    text += answer.slice(last, m.index);
    const n = Number(m[1]);
    if (known.has(n)) {
      flush();
      parts.push({ kind: "cite", n });
    } else {
      text += m[0];
    }
    last = m.index + m[0].length;
  }
  text += answer.slice(last);
  flush();
  return parts;
}

/**
 * Where `quote` sits in `excerpt` as `[start, end)` UTF-16 offsets, or null. The backend verified the
 * quote with case, whitespace and punctuation ignored, so the match does the same: the quote's words
 * (letters and digits) joined by any run of other characters, case insensitive. The range runs from
 * the first word to the last, so surrounding punctuation is not part of the highlight.
 */
export function quoteRange(excerpt: string, quote: string): [number, number] | null {
  const words = quote.split(/[^\p{L}\p{N}]+/u).filter((w) => w !== "");
  if (words.length === 0) return null;
  const re = new RegExp(words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("[^\\p{L}\\p{N}]+"), "iu");
  const m = re.exec(excerpt);
  return m ? [m.index, m.index + m[0].length] : null;
}

/** The excerpt cut into `[before, quote, after]` for highlighting; the quote part is "" when not found. */
export function splitAtQuote(excerpt: string, quote: string | null): [string, string, string] {
  const r = quote ? quoteRange(excerpt, quote) : null;
  if (!r) return [excerpt, "", ""];
  return [excerpt.slice(0, r[0]), excerpt.slice(r[0], r[1]), excerpt.slice(r[1])];
}

const SOURCE_WORDS: Record<AskSource, string> = { note: "Note", task: "Task", clip: "Clipboard" };

export function sourceLabel(source: AskSource): string {
  return SOURCE_WORDS[source];
}

const PLURAL: Record<AskSource, string> = { note: "notes", task: "tasks", clip: "clipboard" };

/** "Looked in notes, tasks and clipboard · 2026-11-05 to 2026-11-08 · 412 items", for the line under an answer. */
export function searchedLine(s: AskSearched): string {
  const where = s.sources.map((x) => PLURAL[x]);
  const list =
    where.length <= 1 ? where.join("") : `${where.slice(0, -1).join(", ")} and ${where[where.length - 1]}`;
  const out = [`Looked in ${list}`];
  if (s.from && s.to) out.push(s.from === s.to ? s.from : `${s.from} to ${s.to}`);
  out.push(`${s.scanned} ${s.scanned === 1 ? "item" : "items"}`);
  if (s.semantic) out.push("by meaning too");
  if (s.rounds > 1) out.push(`${s.rounds} searches`);
  return out.join(" · ");
}

/**
 * An earlier answer as plain text. Its `[n]` markers named items of that search, which are not on
 * screen any more, so they are dropped (with the space before them).
 */
export function plainAnswer(answer: string): string {
  return answer.replace(/\s?\[\d{1,3}\]/g, "");
}

/** The question as it is sent: whitespace trimmed. Null when there is nothing to ask. */
export function cleanQuestion(raw: string): string | null {
  const q = raw.trim();
  return q === "" ? null : q;
}

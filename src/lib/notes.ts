// The Notes view's rules, pure so they are unit tested without a DOM: what shows, in which
// order, and how the filter box and the tag chips share one query string.

import type { Note } from "@/lib/ipc";

/** Mirrors src-tauri/src/notes.rs. */
export const MAX_TITLE_CHARS = 120;
export const MAX_BODY_CHARS = 20_000;
export const MAX_TAGS = 10;
export const MAX_TAG_CHARS = 32;

/** What the list shows for a note without a title: the first line of its body. */
export function noteLabel(n: Pick<Note, "title" | "body">): string {
  if (n.title) return n.title;
  const line = n.body.split("\n").find((l) => l.trim() !== "");
  return line ? line.trim() : "";
}

/** A note with nothing in it at all (left behind by "New note"). */
export function isBlank(n: Pick<Note, "title" | "body" | "tags">): boolean {
  return n.title === "" && n.body.trim() === "" && n.tags.length === 0;
}

/** Same cleaning as the backend, so a tag shows the way it will be stored. */
export function cleanTag(raw: string): string {
  return raw.trim().replace(/^#+/, "").split(/\s+/).filter(Boolean).join("-").toLowerCase();
}

/** Pinned first, then the most recently changed. */
export function sortNotes(notes: readonly Note[]): Note[] {
  return [...notes].sort(
    (a, b) => Number(b.pinned) - Number(a.pinned) || b.updatedAt - a.updatedAt || b.id - a.id,
  );
}

export interface ParsedQuery {
  /** `#tag` words: a note must carry every one of them. */
  tags: string[];
  /** The other words: a note must contain every one (title, body or tag), any case. */
  words: string[];
}

export function parseQuery(query: string): ParsedQuery {
  const tags: string[] = [];
  const words: string[] = [];
  for (const w of query.trim().split(/\s+/).filter(Boolean)) {
    if (w.startsWith("#") && w.length > 1) tags.push(cleanTag(w));
    else words.push(w.toLowerCase());
  }
  return { tags, words };
}

export function filterNotes(notes: readonly Note[], query: string): Note[] {
  const { tags, words } = parseQuery(query);
  const sorted = sortNotes(notes);
  if (tags.length === 0 && words.length === 0) return sorted;
  return sorted.filter((n) => {
    // A tag being typed ("#wo") matches by prefix so the list narrows as you type.
    if (!tags.every((t) => n.tags.some((x) => x === t || x.startsWith(t)))) return false;
    const hay = `${n.title}\n${n.body}\n${n.tags.join(" ")}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

export interface TagCount {
  tag: string;
  count: number;
}

/** Every tag in use with how many notes carry it: most used first, then by name. */
export function tagCounts(notes: readonly Note[]): TagCount[] {
  const m = new Map<string, number>();
  for (const n of notes) for (const t of n.tags) m.set(t, (m.get(t) ?? 0) + 1);
  return [...m].map(([tag, count]) => ({ tag, count })).sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
}

/** Whether the query already filters on `tag`. */
export function hasTagToken(query: string, tag: string): boolean {
  return parseQuery(query).tags.includes(tag);
}

/** Add the `#tag` word to the query, or take it out when it is there (a tag chip click). */
export function toggleTagToken(query: string, tag: string): string {
  const words = query.trim().split(/\s+/).filter(Boolean);
  const isTag = (w: string) => w.startsWith("#") && cleanTag(w) === tag;
  if (words.some(isTag)) return words.filter((w) => !isTag(w)).join(" ");
  return [...words, `#${tag}`].join(" ");
}

/** The selection after the visible list changed: stay on the note if it is still shown. */
export function keepSelection(rows: readonly Note[], selectedId: number | null): number | null {
  if (selectedId !== null && rows.some((n) => n.id === selectedId)) return selectedId;
  return rows[0]?.id ?? null;
}

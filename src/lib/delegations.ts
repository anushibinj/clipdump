// The rules of the Delegations view, pure so they are unit tested without a DOM. The backend
// (`src-tauri/src/delegations.rs`) only stores the document; everything here decides what is valid.

import { relativeTime } from "@/lib/time";

export interface Member {
  id: number;
  name: string;
}

export interface Delegation {
  id: number;
  title: string;
  assigneeId: number;
  /** Unix ms of the follow-up. Always set: nothing can be delegated without one. */
  dueAt: number;
  notes: string;
  /** The `dueAt` an alert was already shown for, so each deadline alerts once (a postpone alerts again). */
  alertedFor: number | null;
  createdAt: number;
}

export interface Doc {
  nextId: number;
  team: Member[];
  items: Delegation[];
}

export const EMPTY_DOC: Doc = { nextId: 1, team: [], items: [] };

export const MAX_NAME_CHARS = 40;
export const MAX_TITLE_CHARS = 120;
export const MAX_NOTES_CHARS = 5000;
const DAY = 86_400_000;

/** The saved text as a document. Empty text is a fresh start; a damaged shape is cleaned, never trusted. */
export function parseDoc(text: string): Doc {
  if (text.trim() === "") return EMPTY_DOC;
  const raw = JSON.parse(text) as Partial<Doc>;
  const team = (Array.isArray(raw.team) ? raw.team : []).filter((m) => typeof m?.name === "string");
  const ids = new Set(team.map((m) => m.id));
  const items = (Array.isArray(raw.items) ? raw.items : []).filter(
    (d) => typeof d?.title === "string" && ids.has(d.assigneeId) && Number.isFinite(d.dueAt),
  );
  const top = Math.max(0, ...team.map((m) => m.id), ...items.map((d) => d.id));
  return { nextId: Math.max(raw.nextId ?? 1, top + 1), team, items };
}

export function addMember(doc: Doc, name: string): Doc {
  const n = name.trim().replace(/\s+/g, " ");
  if (n === "") throw new Error("type a name first");
  if (Array.from(n).length > MAX_NAME_CHARS) throw new Error("that name is too long");
  if (doc.team.some((m) => m.name.toLowerCase() === n.toLowerCase())) throw new Error(`${n} is already on the team`);
  return { ...doc, nextId: doc.nextId + 1, team: [...doc.team, { id: doc.nextId, name: n }] };
}

/** A person with open delegations cannot be removed: their work would lose its owner. */
export function removeMember(doc: Doc, id: number): Doc {
  if (doc.items.some((d) => d.assigneeId === id)) throw new Error("reassign or finish their delegations first");
  return { ...doc, team: doc.team.filter((m) => m.id !== id) };
}

function checkItem(doc: Doc, d: Pick<Delegation, "title" | "assigneeId" | "dueAt">): string {
  const title = d.title.trim();
  if (title === "") throw new Error("give the delegation a title");
  if (Array.from(title).length > MAX_TITLE_CHARS) throw new Error("the title is too long");
  if (!doc.team.some((m) => m.id === d.assigneeId)) throw new Error("pick who it is delegated to");
  if (!Number.isFinite(d.dueAt)) throw new Error("pick a follow-up time");
  return title;
}

export function addItem(doc: Doc, d: Pick<Delegation, "title" | "assigneeId" | "dueAt">, now: number): Doc {
  const title = checkItem(doc, d);
  const item: Delegation = { ...d, title, id: doc.nextId, notes: "", alertedFor: null, createdAt: now };
  return { ...doc, nextId: doc.nextId + 1, items: [...doc.items, item] };
}

/** Change the fields in `patch`; the same rules as `addItem` apply to the result. */
export function updateItem(doc: Doc, id: number, patch: Partial<Pick<Delegation, "title" | "assigneeId" | "dueAt" | "notes">>): Doc {
  const old = doc.items.find((d) => d.id === id);
  if (!old) throw new Error("delegation not found");
  const next = { ...old, ...patch };
  const title = checkItem(doc, next);
  if (Array.from(next.notes).length > MAX_NOTES_CHARS) throw new Error("the notes are too long");
  return { ...doc, items: doc.items.map((d) => (d.id === id ? { ...next, title } : d)) };
}

/** Finished work leaves the list (its notes go with it). */
export function removeItem(doc: Doc, id: number): Doc {
  return { ...doc, items: doc.items.filter((d) => d.id !== id) };
}

export const isOverdue = (d: Delegation, now: number): boolean => d.dueAt <= now;

export const overdueCount = (items: Delegation[], now: number): number => items.filter((d) => isOverdue(d, now)).length;

/** Overdue items nobody was told about yet. */
export const unalerted = (items: Delegation[], now: number): Delegation[] =>
  items.filter((d) => isOverdue(d, now) && d.alertedFor !== d.dueAt);

export function markAlerted(doc: Doc, ids: number[]): Doc {
  return { ...doc, items: doc.items.map((d) => (ids.includes(d.id) ? { ...d, alertedFor: d.dueAt } : d)) };
}

export interface Group {
  member: Member;
  items: Delegation[];
}

/** One group per person who has work, the most urgent first; people without delegations are left out. */
export function groupByAssignee(doc: Doc): Group[] {
  return doc.team
    .map((member) => ({
      member,
      items: doc.items.filter((d) => d.assigneeId === member.id).sort((a, b) => a.dueAt - b.dueAt),
    }))
    .filter((g) => g.items.length > 0)
    .sort((a, b) => a.items[0].dueAt - b.items[0].dueAt);
}

/** "overdue 3h", "in 2d", or "due now". */
export function dueLabel(dueAt: number, now: number): string {
  const span = dueAt <= now ? relativeTime(dueAt, now) : relativeTime(now, dueAt);
  if (span === "now") return "due now";
  return dueAt <= now ? `overdue ${span}` : `in ${span}`;
}

/** The new follow-up for "postpone by N days": counted from now when the old one has passed. */
export const postponed = (dueAt: number, now: number, days: number): number => Math.max(dueAt, now) + days * DAY;

const pad = (n: number) => String(n).padStart(2, "0");

/** A `datetime-local` input value ("2026-11-27T09:30") in local time. */
export function toLocalInput(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Unix ms of a `datetime-local` value, or NaN when it is empty or incomplete. */
export const fromLocalInput = (v: string): number => (v === "" ? NaN : new Date(v).getTime());

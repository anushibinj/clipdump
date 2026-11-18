import { ClipboardList, SearchX, TriangleAlert } from "lucide-react";
import type { TypeFilter } from "@/lib/ipc";

interface EmptyStateProps {
  query: string;
  filter: TypeFilter;
  /** Items in the whole history, or null while unknown. */
  totalItems: number | null;
  /** The last search failed. */
  error: string | null;
}

const FILTER_WORDS: Record<TypeFilter, string> = {
  all: "",
  text: " in text",
  images: " in images",
  files: " in files",
};

/** Shown instead of the rows: empty history, no match, or a failed search. */
export function EmptyState({ query, filter, totalItems, error }: EmptyStateProps) {
  let Icon = SearchX;
  let title: string;
  let hint: string;
  if (error) {
    Icon = TriangleAlert;
    title = "Search is not available";
    hint = error;
  } else if (query.trim() === "" && totalItems === 0) {
    Icon = ClipboardList;
    title = "Nothing copied yet";
    hint = "Copy text, a screenshot or files and it shows up here.";
  } else if (query.trim() === "") {
    title = `No ${filter === "all" ? "items" : filter}`;
    hint = "Press Alt+1 to show every kind of item.";
  } else {
    title = `No matches${FILTER_WORDS[filter]}`;
    hint = "Try fewer letters, or img:, file:, txt: to pick a kind.";
  }
  return (
    <div
      role="status"
      className="flex h-full min-h-40 flex-col items-center justify-center gap-1.5 px-6 text-center"
    >
      <Icon className="mb-1 size-8 text-muted-foreground/60" strokeWidth={1.5} aria-hidden />
      <p className="text-sm font-medium">{title}</p>
      <p className="max-w-72 text-xs text-muted-foreground">{hint}</p>
    </div>
  );
}

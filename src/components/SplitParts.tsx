import { memo, useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { copyText, errorMessage, type SplitPart } from "@/lib/ipc";
import { cn } from "@/lib/utils";

interface SplitPartsProps {
  parts: readonly SplitPart[];
  /** A copy failed: the footer message of the bar. */
  onError: (message: string) => void;
}

/** How long the check mark stays on a copied piece. */
const COPIED_MS = 1200;

/**
 * The pieces of a split text, one row each: what it is, the exact text, a click copies it. The bar
 * stays open so the next piece is one click away (pin the window to keep it while you paste).
 */
export const SplitParts = memo(function SplitParts({ parts, onError }: SplitPartsProps) {
  const [copied, setCopied] = useState<number | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  if (parts.length === 0) {
    return <p className="p-4 text-xs text-muted-foreground">Nothing in this text looks worth copying on its own.</p>;
  }

  const copy = (i: number) => {
    copyText(parts[i].value).then(
      () => {
        setCopied(i);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(null), COPIED_MS);
      },
      (e: unknown) => onError(`Could not copy: ${errorMessage(e)}`),
    );
  };

  return (
    <ScrollArea className="min-h-0 flex-1">
      <ul aria-label="Copyable items" className="divide-y divide-border/60">
        {parts.map((p, i) => (
          <li key={`${i}:${p.value}`}>
            <button
              type="button"
              tabIndex={-1}
              title={`Copy ${p.label}`}
              onClick={() => copy(i)}
              className="group flex w-full items-center gap-2 px-3 py-2 text-left outline-none hover:bg-accent focus-visible:bg-accent"
            >
              <span className="min-w-0 flex-1">
                <span className="block text-[11px] text-muted-foreground">{p.label}</span>
                <span className="block font-mono text-xs break-all">{p.value}</span>
              </span>
              {copied === i ? (
                <Check className="size-3.5 shrink-0 text-emerald-600" aria-label="Copied" />
              ) : (
                <Copy
                  className={cn("size-3.5 shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100")}
                  aria-hidden
                />
              )}
            </button>
          </li>
        ))}
      </ul>
    </ScrollArea>
  );
});

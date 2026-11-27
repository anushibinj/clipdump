import { useCallback, useMemo, useState, type FormEvent, type KeyboardEvent, type RefObject } from "react";
import { ChevronDown, ChevronRight, Plus, UserCheck, Users, X } from "lucide-react";
import { DelegationPanel, fieldClass } from "@/components/DelegationPanel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { DelegationsApi } from "@/hooks/useDelegations";
import {
  MAX_NAME_CHARS,
  MAX_TITLE_CHARS,
  doneItems,
  dueLabel,
  fromLocalInput,
  groupByAssignee,
  isOverdue,
} from "@/lib/delegations";
import { requestAlertPermission } from "@/lib/focusAlert";
import { errorMessage } from "@/lib/ipc";
import { relativeTime } from "@/lib/time";
import { cn } from "@/lib/utils";

interface Props {
  api: DelegationsApi;
  /** The App's one input ref: the bar focuses it when it is shown. */
  inputRef: RefObject<HTMLInputElement | null>;
  onNotice: (kind: "error" | "info", message: string) => void;
  /** Hide the bar. */
  onClose: () => void;
}

/**
 * Work handed to the team, grouped by who has it. A delegation cannot be added without a
 * follow-up time; people with nothing delegated do not show. A click opens the sidebar.
 */
export function DelegationsView({ api, inputRef, onNotice, onClose }: Props) {
  const { doc } = api;
  const [title, setTitle] = useState("");
  const [assignee, setAssignee] = useState("");
  const [due, setDue] = useState("");
  const [name, setName] = useState("");
  const [teamOpen, setTeamOpen] = useState(false);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [showDone, setShowDone] = useState(false);

  const groups = useMemo(() => groupByAssignee(doc), [doc]);
  const done = useMemo(() => doneItems(doc), [doc]);
  const open = doc.items.length - done.length;
  const selected = doc.items.find((d) => d.id === selectedId) ?? null;
  const showTeam = teamOpen || doc.team.length === 0;
  // The assignee select falls back to the only person there is, so a one-person team needs no click.
  const who = assignee !== "" ? assignee : doc.team.length === 1 ? String(doc.team[0].id) : "";

  // A rule broken (blank name, no time...) is a footer message, never a lost change.
  const guard = useCallback(
    (action: () => void) => {
      try {
        action();
      } catch (e) {
        onNotice("error", errorMessage(e));
      }
    },
    [onNotice],
  );

  const submit = (e: FormEvent) => {
    e.preventDefault();
    guard(() => {
      if (who === "") throw new Error("pick who it is delegated to");
      const dueAt = fromLocalInput(due);
      if (Number.isNaN(dueAt)) throw new Error("pick a follow-up time");
      api.add(title, Number(who), dueAt);
      requestAlertPermission(); // from a click or Enter, as browsers require
      setTitle("");
      setDue("");
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (selected) setSelectedId(null);
    else if (teamOpen) setTeamOpen(false);
    else onClose();
  };

  return (
    <div onKeyDown={onKeyDown} className="flex h-full min-h-0 flex-col">
      <form onSubmit={submit} className="flex flex-wrap items-center gap-2 border-b px-4 py-2.5">
        <UserCheck className="size-4 shrink-0 text-muted-foreground" aria-hidden />
        <Input
          ref={inputRef}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="What did you delegate?"
          aria-label="What did you delegate?"
          maxLength={MAX_TITLE_CHARS}
          autoComplete="off"
          spellCheck={false}
          className="h-8 min-w-40 flex-1"
        />
        <select
          value={who}
          onChange={(e) => setAssignee(e.target.value)}
          aria-label="Delegate to"
          className={cn(fieldClass, who === "" && "text-muted-foreground")}
        >
          <option value="" disabled>
            Delegate to…
          </option>
          {doc.team.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </select>
        <input
          type="datetime-local"
          value={due}
          onChange={(e) => setDue(e.target.value)}
          required
          aria-label="Follow up on"
          title="Follow up on (required)"
          className={cn(fieldClass, due === "" && "text-muted-foreground")}
        />
        <Button type="submit" size="sm" disabled={!api.ready || api.error !== null}>
          <Plus aria-hidden />
          Delegate
        </Button>
      </form>

      <div className="flex h-9 items-center gap-3 border-b px-4 text-xs text-muted-foreground">
        <span className="tabular-nums">
          {open} open
        </span>
        {done.length > 0 && (
          <Button type="button" variant="ghost" size="xs" aria-expanded={showDone} onClick={() => setShowDone((v) => !v)} className="gap-1">
            {showDone ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
            Done ({done.length})
          </Button>
        )}
        <Button type="button" variant="ghost" size="xs" aria-pressed={showTeam} onClick={() => setTeamOpen((v) => !v)} className="ml-auto gap-1.5">
          <Users aria-hidden />
          Team ({doc.team.length})
        </Button>
      </div>

      {showTeam && (
        <div className="flex flex-wrap items-center gap-1.5 border-b px-4 py-2">
          {doc.team.map((m) => (
            <span key={m.id} className="inline-flex items-center gap-1 rounded-full bg-foreground/[0.06] py-0.5 pl-2.5 pr-1 text-xs">
              {m.name}
              <button
                type="button"
                aria-label={`Remove ${m.name}`}
                title={`Remove ${m.name}`}
                onClick={() => guard(() => api.removeMember(m.id))}
                className="rounded-full p-0.5 hover:bg-foreground/[0.1]"
              >
                <X className="size-3" aria-hidden />
              </button>
            </span>
          ))}
          <form
            onSubmit={(e) => {
              e.preventDefault();
              guard(() => {
                api.addMember(name);
                setName("");
                setTeamOpen(true); // stays open for the next person once the team is no longer empty
              });
            }}
            className="flex items-center gap-1"
          >
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Add a team member"
              aria-label="Add a team member"
              maxLength={MAX_NAME_CHARS}
              autoComplete="off"
              className="h-7 w-40 text-xs"
            />
            <Button type="submit" variant="outline" size="xs">
              Add
            </Button>
          </form>
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain py-1">
          {groups.length === 0 && !(showDone && done.length > 0) ? (
            <Empty ready={api.ready} error={api.error} hasTeam={doc.team.length > 0} />
          ) : (
            groups.map(({ member, items }) => (
              <section key={member.id} aria-label={member.name} className="py-1">
                <h3 className="px-4 py-1 text-xs font-medium text-muted-foreground">
                  {member.name} <span className="tabular-nums opacity-70">{items.length}</span>
                </h3>
                {items.map((d) => (
                  <button
                    key={d.id}
                    type="button"
                    aria-pressed={d.id === selectedId}
                    onClick={() => setSelectedId(d.id === selectedId ? null : d.id)}
                    className={cn(
                      "flex w-full items-center gap-3 px-4 py-1.5 text-left text-sm transition-colors hover:bg-foreground/[0.05]",
                      d.id === selectedId && "bg-foreground/[0.08]",
                    )}
                  >
                    <span className="min-w-0 flex-1 truncate">{d.title}</span>
                    <span
                      className={cn(
                        "shrink-0 text-xs tabular-nums",
                        isOverdue(d, api.now) ? "font-medium text-red-600 dark:text-red-400" : "text-muted-foreground",
                      )}
                    >
                      {dueLabel(d.dueAt, api.now)}
                    </span>
                  </button>
                ))}
              </section>
            ))
          )}
          {showDone && done.length > 0 && (
            <section aria-label="Done" className="py-1">
              <h3 className="px-4 py-1 text-xs font-medium text-muted-foreground">
                Done <span className="tabular-nums opacity-70">{done.length}</span>
              </h3>
              {done.map((d) => (
                <button
                  key={d.id}
                  type="button"
                  aria-pressed={d.id === selectedId}
                  onClick={() => setSelectedId(d.id === selectedId ? null : d.id)}
                  className={cn(
                    "flex w-full items-center gap-3 px-4 py-1.5 text-left text-sm text-muted-foreground transition-colors hover:bg-foreground/[0.05]",
                    d.id === selectedId && "bg-foreground/[0.08]",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate line-through">{d.title}</span>
                  <span className="shrink-0 text-xs">{doc.team.find((m) => m.id === d.assigneeId)?.name}</span>
                  <span className="shrink-0 text-xs tabular-nums">{relativeTime(d.doneAt ?? 0, api.now)}</span>
                </button>
              ))}
            </section>
          )}
        </div>
        {selected && <DelegationPanel key={selected.id} api={api} item={selected} guard={guard} onClose={() => setSelectedId(null)} />}
      </div>
    </div>
  );
}

function Empty({ ready, error, hasTeam }: { ready: boolean; error: string | null; hasTeam: boolean }) {
  if (!ready) return null;
  return (
    <div className="flex h-full min-h-40 flex-col items-center justify-center gap-1.5 px-6 text-center">
      <UserCheck className="mb-1 size-8 text-muted-foreground/60" strokeWidth={1.5} aria-hidden />
      <p className="text-sm font-medium">{error ? "Delegations are not available" : "Nothing delegated"}</p>
      <p className="max-w-80 text-xs text-muted-foreground">
        {error ?? (hasTeam ? "Delegate something above. Every delegation needs a follow-up time." : "Add the people you delegate to under Team, then delegate something.")}
      </p>
    </div>
  );
}

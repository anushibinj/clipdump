import { useEffect, useState, type ReactNode } from "react";
import { ArrowLeft, FolderOpen, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { bytesToGb, bytesToMb, formatBytes, gbToBytes, mbToBytes } from "@/lib/bytes";
import type { SettingsDto, StatsDto } from "@/lib/ipc";
import type { ThemePref } from "@/lib/theme";

/** Shown next to every control the backend cannot store yet (Phase 12 adds the fields). */
const LATER = "Saved in a later update";

interface SettingsViewProps {
  settings: SettingsDto | null;
  stats: StatsDto | null;
  /** Apply a change. Resolves to an error message, or null on success. */
  onSave: (patch: Partial<SettingsDto>) => Promise<string | null>;
  themePref: ThemePref;
  onThemeChange: (pref: ThemePref) => void;
  onClose: () => void;
}

// ---------------------------------------------------------------------------
// Small building blocks
// ---------------------------------------------------------------------------

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section aria-labelledby={`settings-${title}`} className="px-4 py-3">
      <h3 id={`settings-${title}`} className="mb-1 text-xs font-medium tracking-wide text-muted-foreground uppercase">
        {title}
      </h3>
      <div className="divide-y divide-border/60">{children}</div>
    </section>
  );
}

function Row({
  label,
  hint,
  children,
  htmlFor,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
  htmlFor?: string;
}) {
  return (
    <div className="flex min-h-11 items-center justify-between gap-4 py-2">
      <div className="min-w-0">
        <label htmlFor={htmlFor} className="text-sm">
          {label}
        </label>
        {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

function SwitchRow({
  id,
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  id: string;
  label: string;
  hint?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <Row label={label} hint={disabled ? LATER : hint} htmlFor={id}>
      <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={onChange} />
    </Row>
  );
}

/** A number field that commits on blur or Enter and falls back to the stored value when invalid. */
function NumberRow({
  id,
  label,
  hint,
  value,
  unit,
  step = 1,
  min = 0,
  onCommit,
}: {
  id: string;
  label: string;
  hint?: string;
  value: number;
  unit: string;
  step?: number;
  min?: number;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const n = Number(text);
    if (text.trim() === "" || !Number.isFinite(n) || n < min) {
      setText(String(value));
      return;
    }
    if (n !== value) onCommit(n);
  };
  return (
    <Row label={label} hint={hint} htmlFor={id}>
      <Input
        id={id}
        type="number"
        inputMode="decimal"
        min={min}
        step={step}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
        className="h-8 w-24 text-right tabular-nums"
      />
      <span className="w-12 text-xs text-muted-foreground">{unit}</span>
    </Row>
  );
}

function IgnoredApps({ apps, onChange }: { apps: string[]; onChange: (apps: string[]) => void }) {
  const [draft, setDraft] = useState("");
  const add = () => {
    const name = draft.trim();
    if (name === "") return;
    setDraft("");
    if (!apps.some((a) => a.toLowerCase() === name.toLowerCase())) onChange([...apps, name]);
  };
  return (
    <div className="py-2">
      <label htmlFor="ignored-app" className="text-sm">
        Ignored apps
      </label>
      <p className="text-xs text-muted-foreground">
        Copies from these programs are never captured. Use the exe name, for example keepassxc.exe.
      </p>
      <div className="mt-2 flex gap-2">
        <Input
          id="ignored-app"
          value={draft}
          placeholder="app.exe"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          className="h-8"
        />
        <Button type="button" variant="outline" size="sm" onClick={add} disabled={draft.trim() === ""}>
          Add
        </Button>
      </div>
      {apps.length > 0 && (
        <ul className="mt-2 flex flex-wrap gap-1.5" aria-label="Ignored apps">
          {apps.map((a) => (
            <li key={a}>
              <Badge variant="secondary" className="gap-1 pr-1 font-mono text-[11px] font-normal">
                {a}
                <button
                  type="button"
                  aria-label={`Stop ignoring ${a}`}
                  onClick={() => onChange(apps.filter((x) => x !== a))}
                  className="rounded-full p-0.5 text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
                >
                  <X className="size-3" aria-hidden />
                </button>
              </Badge>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-lg border bg-muted/30 px-3 py-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="text-sm font-medium tabular-nums">{value}</div>
      {sub && <div className="text-xs text-muted-foreground tabular-nums">{sub}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

/**
 * Settings, shown in place of the list (Ctrl+, or the tray). Controls apply at
 * once through `onSave`; numbers commit on blur or Enter. A control whose field
 * the backend does not have yet stays visible but disabled with a short note
 * (hotkey, auto-paste, start at login: Phase 12; data folder: no command yet).
 */
export function SettingsView({ settings, stats, onSave, themePref, onThemeChange, onClose }: SettingsViewProps) {
  const [error, setError] = useState<string | null>(null);
  const save = async (patch: Partial<SettingsDto>) => {
    setError(await onSave(patch));
  };
  const ocrProblem = stats !== null && (stats.ocrEngine === "unavailable" || stats.ocrUnavailable > 0);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <Button type="button" variant="ghost" size="icon" className="size-8" onClick={onClose} aria-label="Back to search">
          <ArrowLeft className="size-4" aria-hidden />
        </Button>
        <h2 className="text-sm font-medium">Settings</h2>
        <span className="ml-auto flex items-center gap-1.5 text-xs text-muted-foreground">
          <Kbd>Esc</Kbd> back
        </span>
      </header>

      {error && (
        <p role="alert" className="border-b bg-destructive/10 px-4 py-2 text-xs text-destructive">
          {error}
        </p>
      )}

      {settings === null ? (
        <p className="p-4 text-sm text-muted-foreground">Loading settings...</p>
      ) : (
        <ScrollArea className="min-h-0 flex-1">
          <Section title="General">
            <Row label="Hotkey" hint={LATER}>
              <KbdGroup>
                <Kbd>Ctrl</Kbd>
                <Kbd>Shift</Kbd>
                <Kbd>Space</Kbd>
              </KbdGroup>
              <Button type="button" variant="outline" size="sm" disabled>
                Change
              </Button>
            </Row>
            <SwitchRow
              id="auto-paste"
              label="Paste on Enter"
              hint="Enter copies and pastes into the window you came from."
              checked={false}
              disabled
              onChange={() => {}}
            />
            <SwitchRow
              id="pinned-bar"
              label="Pinned bar"
              hint="Keep the bar visible; it does not hide when you click elsewhere."
              checked={settings.pinnedBar}
              onChange={(v) => void save({ pinnedBar: v })}
            />
            <SwitchRow id="autostart" label="Start at login" checked={false} disabled onChange={() => {}} />
            <Row label="Theme" htmlFor="theme">
              <ToggleGroup
                id="theme"
                type="single"
                size="sm"
                spacing={1}
                value={themePref}
                onValueChange={(v) => v && onThemeChange(v as ThemePref)}
                aria-label="Theme"
              >
                <ToggleGroupItem value="system" className="h-7 px-2.5 text-xs">
                  System
                </ToggleGroupItem>
                <ToggleGroupItem value="light" className="h-7 px-2.5 text-xs">
                  Light
                </ToggleGroupItem>
                <ToggleGroupItem value="dark" className="h-7 px-2.5 text-xs">
                  Dark
                </ToggleGroupItem>
              </ToggleGroup>
            </Row>
          </Section>
          <Separator />

          <Section title="Capture">
            <SwitchRow
              id="paused"
              label="Pause capture"
              hint="Nothing new is recorded while paused."
              checked={settings.paused}
              onChange={(v) => void save({ paused: v })}
            />
            <SwitchRow
              id="ocr"
              label="Recognize text in images (OCR)"
              hint="Makes words inside screenshots searchable."
              checked={settings.ocrEnabled}
              onChange={(v) => void save({ ocrEnabled: v })}
            />
            {stats && (
              <div className="py-2 text-xs text-muted-foreground" data-testid="ocr-status">
                <span className="text-foreground">Text recognition: </span>
                {stats.ocrEngine === "available"
                  ? stats.ocrBacklog > 0
                    ? `ready, ${stats.ocrBacklog} waiting`
                    : "ready"
                  : stats.ocrEngine === "unavailable"
                    ? "no language installed"
                    : "checking"}
                {ocrProblem && stats.ocrHint !== "" && <p className="mt-1">{stats.ocrHint}</p>}
              </div>
            )}
            <NumberRow
              id="max-text"
              label="Largest text copied"
              value={bytesToMb(settings.maxTextBytes)}
              unit="MB"
              step={0.5}
              min={0.01}
              onCommit={(mb) => void save({ maxTextBytes: mbToBytes(mb) })}
            />
            <NumberRow
              id="max-image"
              label="Largest image copied"
              value={bytesToMb(settings.maxImageBytes)}
              unit="MB"
              step={5}
              min={0.01}
              onCommit={(mb) => void save({ maxImageBytes: mbToBytes(mb) })}
            />
            <IgnoredApps apps={settings.ignoredApps} onChange={(apps) => void save({ ignoredApps: apps })} />
          </Section>
          <Separator />

          <Section title="History">
            <NumberRow
              id="max-items"
              label="Keep at most"
              hint="Oldest unpinned items go first. 0 means no limit."
              value={settings.maxItems}
              unit="items"
              onCommit={(n) => void save({ maxItems: Math.round(n) })}
            />
            <NumberRow
              id="max-age"
              label="Keep for at most"
              hint="Pinned items never expire. 0 means no limit."
              value={settings.maxAgeDays}
              unit="days"
              onCommit={(n) => void save({ maxAgeDays: Math.round(n) })}
            />
            <NumberRow
              id="disk-cap"
              label="Disk cap"
              hint="Oldest unpinned items go first. 0 means no limit."
              value={bytesToGb(settings.diskCapBytes)}
              unit="GB"
              step={0.5}
              onCommit={(gb) => void save({ diskCapBytes: gbToBytes(gb) })}
            />
            <NumberRow
              id="ram-cache"
              label="RAM cache"
              hint="For recently used images and big texts. 0 turns it off."
              value={bytesToMb(settings.ramCacheBytes)}
              unit="MB"
              step={16}
              onCommit={(mb) => void save({ ramCacheBytes: mbToBytes(mb) })}
            />
          </Section>
          <Separator />

          <Section title="Data">
            <Row label="Data folder" hint="Where the encrypted history lives.">
              <Tooltip>
                <TooltipTrigger asChild>
                  {/* A disabled button gets no pointer events, so the span carries the tooltip. */}
                  <span tabIndex={0}>
                    <Button type="button" variant="outline" size="sm" disabled>
                      <FolderOpen aria-hidden /> Open
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent>Not available yet</TooltipContent>
              </Tooltip>
            </Row>
            {stats && (
              <div className="grid grid-cols-2 gap-2 py-3 sm:grid-cols-3" aria-label="Statistics">
                <Stat label="Items" value={String(stats.items)} sub={`${stats.pinned} pinned`} />
                <Stat
                  label="Disk used"
                  value={formatBytes(stats.diskBytes)}
                  sub={settings.diskCapBytes > 0 ? `of ${formatBytes(settings.diskCapBytes)}` : "no cap"}
                />
                <Stat
                  label="RAM cache"
                  value={formatBytes(stats.cacheBytes)}
                  sub={`of ${formatBytes(stats.cacheBudgetBytes)}`}
                />
                <Stat label="Key protection" value={stats.keyProtector} />
              </div>
            )}
          </Section>
        </ScrollArea>
      )}
    </div>
  );
}

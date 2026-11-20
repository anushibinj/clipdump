import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, FolderOpen, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { bytesToGb, bytesToMb, formatBytes, gbToBytes, mbToBytes } from "@/lib/bytes";
import { captureChord, chordParts } from "@/lib/hotkey";
import { errorMessage, openDataFolder, type SettingsDto, type StatsDto } from "@/lib/ipc";
import type { ThemePref } from "@/lib/theme";

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

/** The body of one tab: the tab itself names the group, so there is no heading of its own. */
function Section({ children }: { children: ReactNode }) {
  return <div className="divide-y divide-border/60 px-4 py-2">{children}</div>;
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
  onChange,
}: {
  id: string;
  label: string;
  hint?: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <Row label={label} hint={hint} htmlFor={id}>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
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

/** A text field that commits on blur or Enter, and goes back to the stored value when the save is refused. */
function TextRow({
  id,
  label,
  hint,
  value,
  placeholder,
  type = "text",
  onCommit,
}: {
  id: string;
  label: string;
  hint?: string;
  value: string;
  placeholder?: string;
  type?: "text" | "password";
  onCommit: (value: string) => void;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    if (text.trim() !== value) onCommit(text.trim());
  };
  return (
    <Row label={label} hint={hint} htmlFor={id}>
      <Input
        id={id}
        type={type}
        value={text}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
        className="h-8 w-64 font-mono text-xs"
      />
    </Row>
  );
}

/** A chord as key caps. */
function Chord({ chord }: { chord: string }) {
  return (
    <KbdGroup>
      {chordParts(chord).map((p) => (
        <Kbd key={p}>{p}</Kbd>
      ))}
    </KbdGroup>
  );
}

/**
 * The hotkey row. "Change" starts recording: the next chord is captured with a
 * window level key listener (capture phase, so the settings view's own Esc and
 * Ctrl+, handling never sees these keys), shown live as key caps, and sent to
 * the backend, which registers it. Esc cancels. A refusal (another app owns the
 * chord) is shown right here and the old hotkey stays.
 */
function HotkeyRow({
  chord,
  startupError,
  onChange,
}: {
  chord: string;
  /** Why the saved hotkey is not active (it was taken at startup), or null. */
  startupError: string | null;
  /** Resolves to an error message, or null when the new hotkey is active. */
  onChange: (chord: string) => Promise<string | null>;
}) {
  const [recording, setRecording] = useState(false);
  const [held, setHeld] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const button = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!recording) return;
    const stop = () => {
      setRecording(false);
      setHeld([]);
      button.current?.focus();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      const c = captureChord(e);
      if (c.type === "cancel") {
        setError(null);
        stop();
      } else if (c.type === "pending") {
        setHeld(c.mods);
      } else if (c.type === "invalid") {
        setHeld(c.mods);
        setError(c.reason);
      } else {
        stop();
        setError(null);
        void onChange(c.chord).then(setError);
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, [recording, onChange]);

  const shownError = error ?? startupError;
  return (
    <div>
      <Row label="Hotkey" hint="Shows and hides the bar from anywhere.">
        {recording ? (
          <span className="flex items-center gap-2 text-xs text-muted-foreground" data-testid="hotkey-recording">
            {held.length > 0 ? <Chord chord={held.join("+")} /> : "Press the new shortcut"}
          </span>
        ) : (
          <span data-testid="hotkey-chord">
            <Chord chord={chord} />
          </span>
        )}
        <Button
          ref={button}
          type="button"
          variant="outline"
          size="sm"
          onClick={() => {
            setError(null);
            setHeld([]);
            setRecording((r) => !r);
          }}
          onBlur={() => setRecording(false)}
        >
          {recording ? "Cancel" : "Change"}
        </Button>
      </Row>
      {recording && (
        <p className="pb-2 text-xs text-muted-foreground">
          Hold Ctrl, Alt or Win with a key, then release. <Kbd>Esc</Kbd> cancels.
        </p>
      )}
      {shownError && (
        <p role="alert" data-testid="hotkey-error" className="pb-2 text-xs text-destructive">
          {shownError}
        </p>
      )}
    </div>
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
              <Badge variant="secondary" className="gap-0.5 pr-0.5 font-mono text-[11px] font-normal">
                {a}
                <button
                  type="button"
                  aria-label={`Stop ignoring ${a}`}
                  onClick={() => onChange(apps.filter((x) => x !== a))}
                  // 24 px hit area around the 12 px icon (WCAG 2.5.8).
                  className="-my-1 flex size-6 items-center justify-center rounded-full text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
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
 * once through `onSave` (the backend validates, applies and saves them); numbers
 * commit on blur or Enter. Every control has a backend. The theme is the one
 * setting that stays in the webview (localStorage): it is needed before the
 * first paint.
 */
export function SettingsView({ settings, stats, onSave, themePref, onThemeChange, onClose }: SettingsViewProps) {
  const [error, setError] = useState<string | null>(null);
  const save = async (patch: Partial<SettingsDto>) => {
    setError(await onSave(patch));
  };
  const saveHotkey = (hotkey: string) => onSave({ hotkey });
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
        <p role="alert" className="border-b bg-destructive/10 px-4 py-2 text-xs text-red-700 dark:text-destructive">
          {error}
        </p>
      )}

      {settings === null ? (
        <p className="p-4 text-sm text-muted-foreground">Loading settings...</p>
      ) : (
        <Tabs defaultValue="general" className="min-h-0">
          <TabsList aria-label="Settings groups">
            <TabsTrigger value="general">General</TabsTrigger>
            <TabsTrigger value="capture">Capture</TabsTrigger>
            <TabsTrigger value="ai">AI</TabsTrigger>
            <TabsTrigger value="history">History</TabsTrigger>
            <TabsTrigger value="data">Data</TabsTrigger>
          </TabsList>
          <TabsContent value="general">
            <ScrollArea className="h-full">
              <Section>
              <HotkeyRow
                chord={settings.hotkey}
                startupError={stats?.hotkeyError ?? null}
                onChange={saveHotkey}
              />
              <SwitchRow
                id="auto-paste"
                label="Paste on Enter"
                hint="Enter copies and pastes into the window you came from. Ctrl+Enter only copies."
                checked={settings.autoPasteOnEnter}
                onChange={(v) => void save({ autoPasteOnEnter: v })}
              />
              <SwitchRow
                id="pinned-bar"
                label="Pinned bar"
                hint="Keep the bar visible; it does not hide when you click elsewhere or after a copy."
                checked={settings.pinnedBar}
                onChange={(v) => void save({ pinnedBar: v })}
              />
              <SwitchRow
                id="autostart"
                label="Start at login"
                hint="Starts in the tray when you sign in to Windows."
                checked={settings.startAtLogin}
                onChange={(v) => void save({ startAtLogin: v })}
              />
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
            </ScrollArea>
          </TabsContent>
          <TabsContent value="capture">
            <ScrollArea className="h-full">
              <Section>
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
            </ScrollArea>
          </TabsContent>
          <TabsContent value="ai">
            <ScrollArea className="h-full">
              <Section>
              <p className="py-2 text-xs text-muted-foreground">
                Connect an AI model for the AI features in ClipDump, such as splitting a long text into items
                you can copy one by one. Works with any OpenAI compatible server: OpenAI, Ollama, LM Studio,
                and others. The text of an item is sent to this server only when you use an AI feature on it.
              </p>
              <TextRow
                id="llm-url"
                label="Server URL"
                hint="The API root, for example https://api.openai.com/v1 or http://localhost:11434/v1."
                value={settings.llmBaseUrl}
                placeholder="http://localhost:11434/v1"
                onCommit={(v) => void save({ llmBaseUrl: v })}
              />
              <TextRow
                id="llm-model"
                label="Model"
                value={settings.llmModel}
                placeholder="gpt-4o-mini"
                onCommit={(v) => void save({ llmModel: v })}
              />
              <TextRow
                id="llm-key"
                label="API key"
                hint="Optional for a local server. Stored in settings.json on this computer, unencrypted."
                type="password"
                value=""
                placeholder={settings.llmApiKeySet ? "saved (type to replace)" : "none"}
                onCommit={(v) => void save({ llmApiKey: v })}
              />
              {settings.llmApiKeySet && (
                <div className="flex justify-end pb-2">
                  <Button type="button" variant="ghost" size="xs" onClick={() => void save({ llmApiKey: "" })}>
                    Remove saved key
                  </Button>
                </div>
              )}
              </Section>
            </ScrollArea>
          </TabsContent>
          <TabsContent value="history">
            <ScrollArea className="h-full">
              <Section>
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
            </ScrollArea>
          </TabsContent>
          <TabsContent value="data">
            <ScrollArea className="h-full">
              <Section>
              <Row label="Data folder" hint="Where the encrypted history lives.">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => openDataFolder().then(() => setError(null), (e: unknown) => setError(errorMessage(e)))}
                >
                  <FolderOpen aria-hidden /> Open
                </Button>
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
          </TabsContent>
        </Tabs>
      )}
    </div>
  );
}

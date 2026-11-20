// Telling the user a session or a break is over when they are not looking at the bar. Two quiet
// signals, both best effort (a webview may not offer either): a short tone, and a system
// notification when the user allowed it. Neither may ever throw into the UI.

/** Ask for notification permission. Call it from a click, as browsers require. */
export function requestAlertPermission(): void {
  try {
    if (typeof Notification !== "undefined" && Notification.permission === "default") {
      void Notification.requestPermission().catch(() => undefined);
    }
  } catch {
    // No notifications here: the tone and the in-app message remain.
  }
}

/** A short two-note tone. */
export function beep(): void {
  try {
    const Ctx: typeof AudioContext | undefined =
      window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctx) return;
    const ctx = new Ctx();
    const gain = ctx.createGain();
    gain.gain.value = 0.06;
    gain.connect(ctx.destination);
    [660, 880].forEach((freq, i) => {
      const osc = ctx.createOscillator();
      osc.frequency.value = freq;
      osc.connect(gain);
      osc.start(ctx.currentTime + i * 0.18);
      osc.stop(ctx.currentTime + i * 0.18 + 0.15);
    });
    setTimeout(() => void ctx.close().catch(() => undefined), 800);
  } catch {
    // Audio blocked.
  }
}

/** A system notification, when allowed. */
export function notify(title: string, body: string): void {
  try {
    if (typeof Notification !== "undefined" && Notification.permission === "granted") {
      new Notification(title, { body, silent: true });
    }
  } catch {
    // Not available.
  }
}

/** Tone plus notification. */
export function alert(title: string, body: string): void {
  beep();
  notify(title, body);
}

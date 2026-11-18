// Fake clipboard history for the browser mock (pnpm dev:mock). Pure and
// deterministic: the browser mock backend and the Vite dev server (which
// draws the fake thumbnails) both import it. Never bundled into the app.

export type MockKind = "text" | "image" | "files";

export interface MockFile {
  path: string;
  name: string;
  size: number;
  isDir: boolean;
  exists: boolean;
}

export interface MockItem {
  id: number;
  kind: MockKind;
  /** Full text, the OCR text of an image, or the file names joined by newlines. */
  text: string;
  preview: string;
  files: MockFile[];
  width: number;
  height: number;
  byteSize: number;
  createdAt: number;
  pinned: boolean;
  useCount: number;
  hasThumb: boolean;
}

// Small seeded PRNG (mulberry32) so every run shows the same history.
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const SHORT_TEXTS = [
  "meeting notes with Anna about the Q3 roadmap",
  "https://github.com/tauri-apps/tauri/issues/4881",
  "https://docs.rs/nucleo-matcher/latest/nucleo_matcher/",
  "hello@example.com",
  "npm run build && pnpm tauri build --target x86_64-pc-windows-msvc",
  "C:\\Users\\Sam\\Documents\\invoices\\2026-03\\invoice-0042.pdf",
  "caf\u00e9 au lait, cr\u00e8me br\u00fbl\u00e9e et d\u00e9j\u00e0 vu",
  "\u65e5\u672c\u8a9e\u306e\u30c6\u30ad\u30b9\u30c8\u3092\u30b3\u30d4\u30fc\u3057\u307e\u3057\u305f",
  "Ship it \u{1F680} then celebrate \u{1F389} with the whole team",
  "SELECT id, name, created_at FROM customers WHERE active = 1 ORDER BY created_at DESC LIMIT 50;",
  "+49 30 1234 5678",
  "4111 1111 1111 1111 (test card, do not use)",
  "The quick brown fox jumps over the lazy dog",
  "TODO: rename the clipboard watcher thread and add a regression test",
  "ssh deploy@build-07.internal -p 2222 -i ~/.ssh/id_ed25519",
  "git rebase -i HEAD~5 --autosquash",
  "rgb(24, 24, 27) / #18181b / oklch(0.21 0.006 285.885)",
  "Invoice 2026-0183 due on 14 December, total 1,284.50 EUR",
  "Please find the updated slide deck attached, thanks again for the quick review.",
  "const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));",
  "kubectl rollout restart deployment/api -n production",
  "Meeting link: https://meet.example.com/abc-defg-hij",
  "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
  "lorem ipsum dolor sit amet, consectetur adipiscing elit",
  "Re: budget approval for the new design system licenses",
  "192.168.1.42:8080",
  "pnpm add -D vitest @vitest/coverage-v8",
  "Build succeeded in 12.4s with 0 warnings",
];

const MULTI_TEXTS = [
  `fn main() {
    let items = load_history()?;
    for item in items.iter().filter(|i| i.pinned) {
        println!("{}", item.preview);
    }
}`,
  `{
  "name": "clipdump",
  "version": "0.1.0",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build"
  }
}`,
  `Hi team,

Quick update on the release: the installer is signed, the changelog is in
place and the staging build passed all smoke tests. We plan to ship on Tuesday
unless someone objects.

Best,
Sam`,
  `- [x] capture text, images and files
- [x] OCR with Windows.Media.Ocr
- [ ] auto-paste into the previous window
- [ ] installer without admin rights`,
  `def fuzzy_score(needle: str, hay: str) -> float:
    """Return a score in 0..1 for how well needle matches hay."""
    if not needle:
        return 1.0
    hits = sum(1 for c in needle if c in hay)
    return hits / len(needle)`,
  `Shopping list
milk
oat flour
two lemons
dark chocolate
coffee beans`,
  `SELECT c.name, COUNT(o.id) AS orders
FROM customers c
LEFT JOIN orders o ON o.customer_id = c.id
GROUP BY c.name
HAVING COUNT(o.id) > 3
ORDER BY orders DESC;`,
  `Error: connect ECONNREFUSED 127.0.0.1:5432
    at TCPConnectWrap.afterConnect [as oncomplete] (node:net:1555:16)
    at Object.connect (/app/node_modules/pg/lib/client.js:132:7)
    at runMicrotasks (<anonymous>)`,
];

const LONG_TEXT = (n: number) =>
  Array.from({ length: 6 + (n % 5) }, (_, i) =>
    `Paragraph ${i + 1}. The history keeps text, screenshots and file references, ` +
    `searchable by content. Retention caps the unpinned items and their age, ` +
    `and pinned items stay at the top (variant ${n}).`,
  ).join("\n\n");

const OCR_TEXTS = [
  "Settings\nNetwork\nWi-Fi connected\nIP address 192.168.1.42\nDNS 1.1.1.1\nProxy: off",
  "Error 500\nInternal Server Error\nThe server encountered an unexpected condition.\nRequest id 7f3a-91bc-44d0",
  "Quarterly revenue\nQ1 1,204,000\nQ2 1,388,500\nQ3 1,512,250\nTotal 4,104,750 EUR",
  "Order confirmation\nThank you for your purchase\nOrder number 88213-A\nEstimated delivery 3 to 5 days",
  "File Edit View Help\nmain.rs\nfn handle_clipboard_update()\nlet text = read_text()?;\nstore.insert(text);",
  "Meeting at 14:30\nRoom Hamburg 2.14\nAgenda: roadmap review, hiring, budget\nBring your laptop",
  "Password reset\nWe sent a link to your email address\nThe link expires in 30 minutes\nDidn't get it? Resend",
  "Boarding pass\nFlight LH 402\nFrankfurt to Lisbon\nGate B42 boarding 09:15\nSeat 14C",
  "package.json\ndependencies\nreact 19.1.0\ntailwindcss 4.3.5\nvite 8.0.16\ncmdk 1.1.1",
  "Caf\u00e9 receipt\nFlat white 3.40\nCroissant 2.90\nTotal 6.30 EUR\nThank you, see you soon",
];

const FILE_SETS: { names: string[]; dir: string; sizes: number[]; missing?: number[] }[] = [
  { dir: "C:\\Users\\Sam\\Documents", names: ["report-final.pdf"], sizes: [2_480_000] },
  { dir: "C:\\Users\\Sam\\Downloads", names: ["setup.exe", "readme.txt", "license.txt", "data.zip"], sizes: [48_000_000, 4_200, 11_300, 120_400_000] },
  { dir: "D:\\Photos\\2026\\Lisbon", names: ["IMG_4021.JPG", "IMG_4022.JPG", "IMG_4023.JPG"], sizes: [3_800_000, 4_100_000, 3_950_000] },
  { dir: "C:\\Projects\\clipdump", names: ["src", "package.json", "PLAN.md"], sizes: [0, 1_200, 18_400] },
  { dir: "C:\\Users\\Sam\\Videos", names: ["demo-recording.mp4"], sizes: [182_000_000], missing: [0] },
  { dir: "E:\\Backup", names: ["old-notes.docx", "budget-2024.xlsx"], sizes: [88_000, 412_000], missing: [1] },
  { dir: "C:\\Users\\Sam\\Music", names: ["track-01.flac", "track-02.flac", "track-03.flac", "track-04.flac", "track-05.flac"], sizes: [31_000_000, 28_500_000, 33_100_000, 29_900_000, 30_200_000] },
];

function fileItem(n: number): { files: MockFile[]; byteSize: number; hasThumb: boolean } {
  const set = FILE_SETS[n % FILE_SETS.length];
  const files: MockFile[] = set.names.map((name, i) => ({
    path: `${set.dir}\\${name}`,
    name,
    size: set.sizes[i],
    isDir: name === "src",
    exists: !(set.missing ?? []).includes(i),
  }));
  const byteSize = files.reduce((a, f) => a + f.size, 0);
  const hasThumb = /\.(jpg|png)$/i.test(set.names[0]);
  return { files, byteSize, hasThumb };
}

function collapse(s: string): string {
  return s.replace(/\s+/g, " ").trim().slice(0, 300);
}

const ID_BASE = 100_000;

/**
 * `count` items, newest first, ids descending from 100,000. Mix: about 55%
 * text (short, multi-line, long), 25% screenshots with OCR text, 20% file
 * lists; a handful are pinned.
 */
export function generateItems(count: number, now: number = Date.now(), seed = 7): MockItem[] {
  const rand = rng(seed);
  const items: MockItem[] = [];
  let createdAt = now - 20_000;
  for (let i = 0; i < count; i += 1) {
    // Ids do not depend on `count`, so the dev server (which draws the pictures) and
    // the page agree on which id is which item however many items the page loads.
    const id = ID_BASE - i;
    // Gaps grow with age: minutes at first, hours and days further back.
    createdAt -= Math.round((5_000 + rand() * 120_000) * (1 + i / 12));
    const roll = rand();
    const pinned = i === 3 || i === 11 || i === 37 || (i > 60 && i % 97 === 0);
    const useCount = Math.floor(rand() * 6);
    const base = { id, createdAt, pinned, useCount };
    if (roll < 0.55) {
      const sub = rand();
      let text: string;
      if (sub < 0.5) text = SHORT_TEXTS[Math.floor(rand() * SHORT_TEXTS.length)];
      else if (sub < 0.85) text = MULTI_TEXTS[Math.floor(rand() * MULTI_TEXTS.length)];
      else text = LONG_TEXT(id);
      // Make repeats distinct so the list is not a visible loop.
      if (i >= SHORT_TEXTS.length && sub < 0.5) text = `${text} (${id})`;
      items.push({
        ...base,
        kind: "text",
        text,
        preview: collapse(text),
        files: [],
        width: 0,
        height: 0,
        byteSize: new TextEncoder().encode(text).length,
        hasThumb: false,
      });
    } else if (roll < 0.8) {
      const ocr = OCR_TEXTS[Math.floor(rand() * OCR_TEXTS.length)];
      const sizes = [
        [1920, 1080],
        [2560, 1440],
        [1280, 720],
        [800, 600],
        [3840, 2160],
      ];
      const [width, height] = sizes[Math.floor(rand() * sizes.length)];
      items.push({
        ...base,
        kind: "image",
        text: ocr,
        preview: `Image ${width}x${height}`,
        files: [],
        width,
        height,
        byteSize: Math.round(width * height * (0.18 + rand() * 0.25)),
        hasThumb: true,
      });
    } else {
      const f = fileItem(Math.floor(rand() * 1000));
      items.push({
        ...base,
        kind: "files",
        text: f.files.map((x) => x.name).join("\n"),
        preview: collapse(f.files.map((x) => x.name).join(", ")),
        files: f.files,
        width: 0,
        height: 0,
        byteSize: f.byteSize,
        hasThumb: f.hasThumb,
      });
    }
  }
  return items;
}

/** Fake thumbnail and full image as SVG text, for the Vite dev server. */
export function fakePicture(item: MockItem, full: boolean): string {
  const hue = (item.id * 47) % 360;
  const w = full ? 1280 : Math.round((96 * (item.width || 4)) / (item.height || 3));
  const h = full ? Math.round((1280 * (item.height || 3)) / (item.width || 4)) : 96;
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const lines = item.kind === "image" ? item.text.split("\n") : [item.preview];
  const fs = Math.max(8, Math.round(h / 14));
  const text = lines
    .slice(0, 9)
    .map(
      (l, i) =>
        `<text x="${Math.round(w * 0.06)}" y="${Math.round(h * 0.28 + i * fs * 1.5)}" font-size="${fs}" font-family="sans-serif" fill="hsl(${hue} 20% 25%)">${esc(l)}</text>`,
    )
    .join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
<rect width="${w}" height="${h}" fill="hsl(${hue} 40% 94%)"/>
<rect width="${w}" height="${Math.round(h * 0.12)}" fill="hsl(${hue} 45% 62%)"/>
<circle cx="${Math.round(w * 0.03)}" cy="${Math.round(h * 0.06)}" r="${Math.round(h * 0.025)}" fill="#fff" opacity=".8"/>
${text}</svg>`;
}

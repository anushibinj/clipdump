import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// Phase 2 shell: a blank rounded card that tracks bar visibility. The real
// search UI arrives in Phase 10.
function App() {
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const unlisten: Promise<UnlistenFn>[] = [];
    try {
      unlisten.push(listen("bar:shown", () => setShown(true)));
      unlisten.push(listen("bar:hidden", () => setShown(false)));
    } catch {
      // Plain browser (pnpm dev without Tauri): no event bus, stay blank.
    }
    return () => {
      unlisten.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, []);

  return (
    // 8 px padding leaves room for the native window shadow and the card shadow.
    <div className="h-screen w-screen p-2">
      <main
        data-shown={shown}
        className="h-full w-full rounded-xl border border-border bg-popover/95 shadow-lg"
      />
    </div>
  );
}

export default App;

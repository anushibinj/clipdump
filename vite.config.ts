import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";
import process from "node:process";
import { fakePicture, generateItems } from "./src/dev/data";
const host = process.env.TAURI_DEV_HOST;

/** Dev server only (mock mode): serves fake thumbnails and full images as SVG. */
function mockClipAssets(): Plugin {
  const items = new Map(generateItems(1000).map((i) => [i.id, i] as const));
  return {
    name: "clipdump-mock-clip-assets",
    configureServer(server) {
      server.middlewares.use("/mock-clip", (req, res) => {
        const m = /^\/(thumb|blob)\/(\d+)/.exec(req.url ?? "");
        const item = m ? items.get(Number(m[2])) : undefined;
        if (!m || !item || !item.hasThumb) {
          res.statusCode = 404;
          res.end();
          return;
        }
        res.setHeader("content-type", "image/svg+xml");
        res.setHeader("cache-control", "max-age=31536000, immutable");
        res.end(fakePicture(item, m[1] === "blob"));
      });
    },
  };
}

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss(), ...(mode === "mock" ? [mockClipAssets()] : [])],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
      // `vite --mode mock` runs the UI in a plain browser against a fake
      // backend (src/dev). Only this mode aliases the Tauri bridge, so none of
      // the mock code can reach a production bundle.
      ...(mode === "mock"
        ? {
            "@tauri-apps/api/core": path.resolve(import.meta.dirname, "./src/dev/mock-core.ts"),
            "@tauri-apps/api/event": path.resolve(import.meta.dirname, "./src/dev/mock-event.ts"),
          }
        : {}),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));

import path from "node:path";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue(), tailwindcss()],

  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
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
    // Makes the API same-origin with the dev server, which is what keeps the
    // SameSite=Strict refresh cookie working and lets us skip CORS entirely.
    // Prod does the same job with Caddy. This is server-side (runs in the vite
    // node process), so `localhost` resolves on this machine even when the page
    // is loaded from a phone over the LAN during `tauri android dev`.
    // Don't set changeOrigin: the Host header is what binds the proxied
    // Set-Cookie to the dev origin.
    proxy: {
      "/api/user": "http://localhost:3000",
      "/api/booking": "http://localhost:3001",
      "/api/spot": "http://localhost:3002",
      "/api/view": "http://localhost:3003",
      "/api/media": "http://localhost:3004",
      "/api/payment": "http://localhost:3006",
    },
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

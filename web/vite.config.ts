import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In development the API runs on :8080; proxy it (including the WebSocket) so the app
// uses same-origin relative URLs. For production set VITE_API_BASE at build time.
const api = process.env.BINSCOPE_API ?? "http://localhost:8080";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: { "/v1": { target: api, changeOrigin: true, ws: true } },
  },
  preview: {
    port: 4173,
    proxy: { "/v1": { target: api, changeOrigin: true, ws: true } },
  },
  build: { target: "es2022", sourcemap: true },
});

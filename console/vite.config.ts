import path from "node:path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    // rename from default `assets` so the SPA route /assets/* doesn't
    // collide with the static bundle dir (nginx serves the real dir and
    // autoindex 403s instead of falling through to index.html on /assets/).
    assetsDir: "static",
  },
})

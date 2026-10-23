import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1425,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/.local/**", "**/release/**", "**/dist/**"] },
  },
  build: { target: "es2022" },
});

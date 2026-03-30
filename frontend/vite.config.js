import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "build",
    lib: {
      entry: "src/index.js",
      formats: ["es"],
      fileName: "index",
    },
    rollupOptions: {
      external: [
        "$lib/plugin-invoke",
        "$lib/i18n",
        "$lib/stores/download-store.svelte",
        "$lib/stores/settings-store.svelte",
        "$lib/stores/toast-store.svelte",
        "$app/navigation",
        "@tauri-apps/api/core",
        "@tauri-apps/api/event",
        "@tauri-apps/plugin-dialog",
        "svelte",
      ],
    },
  },
});

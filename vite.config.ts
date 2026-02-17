import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: "./",
  clearScreen: false,
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return;

          if (id.includes("@xterm/")) return "vendor-xterm";
          if (id.includes("@tauri-apps/")) return "vendor-tauri";
          if (id.includes("react") || id.includes("scheduler")) return "vendor-react";
          if (id.includes("@radix-ui/")) return "vendor-radix";
          if (id.includes("@dnd-kit/")) return "vendor-dnd-kit";
          if (id.includes("lucide-react")) return "vendor-icons";

          return "vendor";
        },
      },
    },
  },
  server: {
    // Bind to all interfaces so the WebView can always reach the dev server.
    // HMR still points at 127.0.0.1 to avoid IPv6/hostname quirks on macOS.
    host: true,
    port: 1420,
    strictPort: true,
    origin: "http://127.0.0.1:1420",
    hmr: {
      host: "127.0.0.1",
      protocol: "ws",
    },
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
});

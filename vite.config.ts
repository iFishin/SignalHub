import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

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

  test: {
    // 这个仓库在外接卷上（exFAT），macOS 会给每个文件配一个 `._<原名>` 的
    // AppleDouble 元数据文件。它和源文件**同名同后缀**，vitest 的默认 glob 会
    // 把它当成测试文件去解析，报一个和测试毫无关系的 `PARSE_ERROR: Invalid
    // Character`。`.gitignore` 里的 `._*` 拦得住 git，拦不住 glob —— 所以这里
    // 再排除一次。
    exclude: ["**/node_modules/**", "**/dist/**", "**/._*"],
    css: true,
  },
}));

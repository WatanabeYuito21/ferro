import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// Tauriが期待する固定ポート・厳密ポートに合わせる設定。
// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      // src-tauriのビルド成果物への変更でフロントのHMRが誤爆しないようにする。
      ignored: ['**/src-tauri/**'],
    },
  },
})

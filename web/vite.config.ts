import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Long-lived vendor code sits in its own chunks so page releases do not
// invalidate it in browser caches.
const vendor = (id: string) => {
  if (/node_modules\/(react|react-dom|scheduler)\//.test(id)) return 'react'
  if (/node_modules\/(radix-ui|@radix-ui|@floating-ui|react-remove-scroll|react-remove-scroll-bar|react-style-singleton|use-callback-ref|use-sidecar|aria-hidden|get-nonce|tslib)\//.test(id)) return 'radix'
  if (/node_modules\/(motion|motion-dom|motion-utils|framer-motion)\//.test(id)) return 'motion'
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': '/src' } },
  server: { proxy: { '/api': 'http://127.0.0.1:8080', '/sub': 'http://127.0.0.1:8080' } },
  build: { sourcemap: false, rollupOptions: { output: { manualChunks: vendor } } },
})

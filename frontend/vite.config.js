import { fileURLToPath, URL } from 'node:url'
import { execSync } from 'node:child_process'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

function getVersion() {
  if (process.env.PHOS_VERSION) return process.env.PHOS_VERSION
  try {
    return execSync('git describe --tags --exact-match', { encoding: 'utf-8' }).trim()
  } catch {}
  try {
    return execSync('git symbolic-ref --short HEAD', { encoding: 'utf-8' }).trim()
  } catch {}
  try {
    return 'sha-' + execSync('git rev-parse --short HEAD', { encoding: 'utf-8' }).trim()
  } catch {}
  return 'unknown'
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    vue(),
    tailwindcss(),
  ],
  define: {
    __PHOS_VERSION__: JSON.stringify(getVersion()),
  },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url))
    }
  },
  css: {
    transformer: 'lightningcss'
  },
  build: {
    cssMinify: 'lightningcss'
  }
})
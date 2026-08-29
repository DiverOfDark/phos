<script setup>
import { ref, onMounted, computed, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuth } from '@/composables/useAuth'

const version = __PHOS_VERSION__

const { user, authEnabled, logout } = useAuth()
// "local" when the server runs without an identity provider — a "?" avatar reads
// as a failed lookup rather than as single-user mode.
const userDisplayName = computed(() =>
  authEnabled.value && user.value
    ? (user.value.name || user.value.email || user.value.sub || 'user')
    : 'local'
)

const route = useRoute()
const router = useRouter()
const isLogin = computed(() => route.name === 'login')
const currentView = computed(() => route.meta.view || 'overview')

// --- Pending count for the Review Desk badge ---
const pendingCount = ref(0)

async function fetchPendingCount() {
  try {
    const res = await fetch('/api/organize/stats')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    pendingCount.value = data.pending_review || 0
  } catch {
    pendingCount.value = 0
  }
}

const navItems = computed(() => [
  { view: 'overview', label: 'Overview', to: '/' },
  { view: 'review', label: 'Review Desk', to: '/review', badge: pendingCount.value || null },
  { view: 'people', label: 'People', to: '/people' },
  { view: 'workflows', label: 'Workflows', to: '/workflows' },
  { view: 'settings', label: 'Settings', to: '/settings' },
])

// --- Import ---
//
// Not a station on the design's route diagram, but Phos can ingest files the
// scanner never sees, so the dialog stays — reachable from the sidebar foot.
const showImportDialog = ref(false)
const isDragging = ref(false)
const importPath = ref('')
const importMessage = ref('')
const importError = ref('')
const isImporting = ref(false)
const libraryPath = ref(localStorage.getItem('phos_library_path') || '/mnt/photos')

const routeComponentRef = ref(null)

function refreshViews() {
  const comp = routeComponentRef.value
  if (comp) {
    comp.loadData?.()
    comp.fetchPhotos?.()
    comp.fetchPeople?.()
    comp.fetchShots?.()
  }
}

const handleImportScan = async () => {
  const pathToScan = importPath.value.trim() || libraryPath.value
  if (!pathToScan) return

  isImporting.value = true
  importMessage.value = ''
  importError.value = ''

  try {
    const response = await fetch('/api/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: pathToScan }),
    })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    importMessage.value = `scan queued · ${pathToScan}`
    setTimeout(async () => {
      await fetchPendingCount()
      refreshViews()
      isImporting.value = false
    }, 5000)
  } catch (e) {
    console.error('Import scan failed:', e)
    importError.value = e.message || 'scan request failed — is the backend running?'
    isImporting.value = false
  }
}

const uploadProgress = ref({ current: 0, total: 0 })
const isUploading = ref(false)
// Analysis (faces, thumbnails, embeddings) happens on the server after the
// upload responds, so the dialog reports it separately: uploading is over in
// seconds, indexing is the part that takes minutes on a big drop.
const analyzeProgress = ref({ remaining: 0, total: 0 })
const isAnalyzing = ref(false)

async function fetchIngestStatus() {
  const res = await fetch('/api/import/status')
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return await res.json()
}

/**
 * Poll the ingest queue until it drains, refreshing the views as it goes.
 *
 * The refresh mid-flight is the point: photos appear in the gallery as they are
 * analyzed instead of all at once at the end, so a long import looks like
 * progress rather than a stalled dialog.
 */
async function trackAnalysis() {
  isAnalyzing.value = true
  let peak = 0
  try {
    for (;;) {
      const status = await fetchIngestStatus()
      const remaining = (status.queued || 0) + (status.analyzing || 0)
      peak = Math.max(peak, remaining)
      analyzeProgress.value = { remaining, total: peak }
      refreshViews()
      if (remaining === 0) break
      await new Promise((resolve) => setTimeout(resolve, 1500))
    }
  } catch (e) {
    // The files are uploaded either way; only the progress readout is lost.
    console.error('Failed to poll import status:', e)
  } finally {
    isAnalyzing.value = false
    analyzeProgress.value = { remaining: 0, total: 0 }
  }
}

const handleDrop = async (e) => {
  isDragging.value = false
  importMessage.value = ''
  importError.value = ''

  const files = Array.from(e.dataTransfer?.files || [])
  const mediaFiles = files.filter(f => /\.(jpe?g|png|webp|mp4|mov|mkv|avi|webm)$/i.test(f.name))
  if (mediaFiles.length === 0) {
    importError.value = 'no supported media files — jpeg, png, webp, mp4, mov, mkv, avi, webm'
    return
  }

  isUploading.value = true
  uploadProgress.value = { current: 0, total: mediaFiles.length }
  let failed = 0

  for (const file of mediaFiles) {
    try {
      const res = await fetch(`/api/import/upload?filename=${encodeURIComponent(file.name)}`, {
        method: 'PUT',
        body: file,
      })
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
    } catch (e) {
      console.error(`Failed to upload ${file.name}:`, e)
      failed++
    }
    uploadProgress.value.current++
  }

  isUploading.value = false
  const succeeded = mediaFiles.length - failed
  importMessage.value = failed === 0
    ? `${succeeded} file(s) uploaded · indexing in the background`
    : `${succeeded} of ${mediaFiles.length} uploaded · indexing in the background`
  if (failed > 0) importError.value = `${failed} file(s) failed to upload`

  await fetchPendingCount()
  refreshViews()
  await trackAnalysis()
  await fetchPendingCount()
}

onMounted(() => {
  fetchPendingCount()
})
</script>

<template>
  <!-- Login stands alone: no shell, no nav. -->
  <router-view v-if="isLogin" />

  <div v-else class="min-h-screen flex flex-col md:flex-row bg-base text-ink">
    <!-- Sidebar (desktop) -->
    <div class="hidden md:flex w-52 flex-none border-r border-line flex-col sticky top-0 h-screen">
      <router-link to="/" class="flex items-center gap-2 p-4 border-b border-line">
        <img src="/phos.svg" alt="" class="w-6 h-6 rounded-[4px]" />
        <span class="font-heading text-base font-bold tracking-[-0.01em] text-ink">Phos</span>
      </router-link>

      <nav class="flex flex-col py-4 gap-0.5 flex-1">
        <router-link
          v-for="n in navItems"
          :key="n.view"
          :to="n.to"
          class="flex items-center gap-2 px-4 py-2 border-l-2 text-[13px] transition-colors"
          :class="currentView === n.view
            ? 'bg-surface border-signal text-ink font-medium'
            : 'border-transparent text-ink-secondary hover:text-ink'"
        >
          <span class="flex-1">{{ n.label }}</span>
          <span v-if="n.badge" class="font-mono text-[11px] text-degraded">{{ n.badge }}</span>
        </router-link>

        <button
          class="mt-4 mx-4 px-3 py-2 border border-dashed border-line-strong rounded text-[13px] text-ink-secondary hover:text-signal transition-colors text-left"
          @click="showImportDialog = true"
        >
          Import…
        </button>
      </nav>

      <div class="p-4 border-t border-line flex items-center gap-2">
        <div class="w-6 h-6 rounded bg-raised border border-line flex items-center justify-center font-mono text-[11px] text-ink-secondary uppercase">
          {{ userDisplayName.charAt(0) }}
        </div>
        <div class="flex-1 min-w-0">
          <div class="text-xs text-ink truncate">{{ userDisplayName }}</div>
        </div>
        <button
          v-if="authEnabled && user"
          class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
          @click="logout"
        >exit</button>
      </div>
    </div>

    <div class="flex-1 min-w-0 flex flex-col">
      <!-- Topbar (mobile) -->
      <div class="md:hidden h-14 flex-none border-b border-line flex items-center gap-4 px-4 sticky top-0 bg-base z-10">
        <router-link to="/" class="flex items-center gap-2">
          <img src="/phos.svg" alt="" class="w-6 h-6 rounded-[4px]" />
          <span class="font-heading text-base font-bold text-ink">Phos</span>
        </router-link>
        <div class="flex-1"></div>
        <button class="font-mono text-[11px] text-ink-tertiary hover:text-signal" @click="showImportDialog = true">import</button>
        <button v-if="authEnabled && user" class="font-mono text-[11px] text-ink-tertiary hover:text-signal" @click="logout">exit</button>
      </div>

      <!-- Lane tabs (mobile nav) -->
      <div class="md:hidden flex-none border-b border-line flex items-center overflow-x-auto">
        <router-link
          v-for="n in navItems"
          :key="n.view"
          :to="n.to"
          class="flex items-center gap-1.5 px-3 h-11 border-b-2 text-[13px] whitespace-nowrap transition-colors"
          :class="currentView === n.view
            ? 'border-signal text-ink font-medium'
            : 'border-transparent text-ink-secondary'"
        >
          {{ n.label }}
          <span v-if="n.badge" class="font-mono text-[11px] text-degraded">{{ n.badge }}</span>
        </router-link>
      </div>

      <div class="flex-1 min-h-0 flex flex-col">
        <router-view v-slot="{ Component }">
          <component :is="Component" ref="routeComponentRef" />
        </router-view>
      </div>
    </div>

    <!-- Import dialog -->
    <div
      v-if="showImportDialog"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
      style="background: var(--scrim)"
      @click="showImportDialog = false"
    >
      <div
        class="w-[480px] max-w-full max-h-[calc(100vh-64px)] bg-overlay border border-line-strong rounded shadow-lg flex flex-col overflow-hidden"
        @click.stop
      >
        <div class="flex items-center justify-between px-6 py-4 border-b border-line">
          <div class="font-heading text-base font-semibold text-ink">Import media</div>
          <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="showImportDialog = false">✕</button>
        </div>

        <div class="p-6 flex flex-col gap-6 overflow-y-auto min-h-0">
          <div class="flex flex-col gap-2">
            <div class="label">Scan a server directory</div>
            <div class="flex gap-2">
              <input
                v-model="importPath"
                :placeholder="libraryPath"
                spellcheck="false"
                class="flex-1 bg-base border border-line rounded-sm px-3 py-2 font-mono text-[13px] text-ink"
              />
              <button
                class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
                :disabled="isImporting"
                @click="handleImportScan"
              >{{ isImporting ? 'Scanning…' : 'Scan' }}</button>
            </div>
            <div class="text-xs font-light text-ink-secondary">Leave empty to use the library path.</div>
          </div>

          <div
            class="flex flex-col items-center justify-center gap-2 border border-dashed rounded p-8 transition-colors"
            :class="isDragging ? 'border-signal bg-surface' : 'border-line-strong'"
            @dragover.prevent="isDragging = true"
            @dragleave.prevent="isDragging = false"
            @drop.prevent="handleDrop"
          >
            <template v-if="isUploading">
              <div class="font-mono text-[13px] text-ink">uploading {{ uploadProgress.current }} / {{ uploadProgress.total }}</div>
            </template>
            <template v-else-if="isAnalyzing && analyzeProgress.remaining > 0">
              <div class="font-mono text-[13px] text-ink">
                indexing {{ analyzeProgress.total - analyzeProgress.remaining }} / {{ analyzeProgress.total }}
                <span class="text-building signal-pulse">●</span>
              </div>
              <div class="text-xs font-light text-ink-secondary text-center">
                Uploads are done. Face detection continues in the background — this dialog can be closed.
              </div>
            </template>
            <template v-else>
              <div class="font-mono text-[13px] text-ink-secondary">drop files to upload</div>
              <div class="text-xs font-light text-ink-secondary">jpeg · png · webp · mp4 · mov · mkv · avi · webm</div>
            </template>
          </div>

          <div v-if="importMessage" class="font-mono text-xs text-ready">{{ importMessage }}</div>
          <div v-if="importError" class="font-mono text-xs text-error">{{ importError }}</div>
        </div>

        <div class="border-t border-line px-6 py-3 flex items-center">
          <span class="flex-1"></span>
          <span class="font-mono text-[11px] text-ink-tertiary">phos {{ version }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style>
/* Track lines, not scroll furniture. */
::-webkit-scrollbar { width: 8px; height: 8px }
::-webkit-scrollbar-track { background: transparent }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 2px }
::-webkit-scrollbar-thumb:hover { background: var(--border-strong) }
</style>

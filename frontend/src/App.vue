<script setup>
import { ref, onMounted, computed, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { cn } from '@/lib/utils'
import { useAuth } from '@/composables/useAuth'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
  SheetTrigger,
} from '@/components/ui/sheet'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'

import {
  Settings,
  Search,
  Upload,
  LayoutGrid,
  RefreshCw,
  FolderOpen,
  Check,
  AlertCircle,
  ClipboardCheck,
  Users,
  LogOut,
  Wand2,
  Layers,
  Globe,
  Trash2,
  Eye,
  EyeOff,
  Copy,
} from 'lucide-vue-next'

// --- Version ---
const version = __PHOS_VERSION__

// --- Auth ---
const { user, authEnabled, logout } = useAuth()
const userDisplayName = computed(() => user.value?.name || user.value?.email || user.value?.sub || '?')

// --- Router ---
const route = useRoute()
const router = useRouter()

const currentView = computed(() => route.meta.view || 'organize')

// --- Pending count for Review badge ---
const pendingCount = ref(0)

async function fetchPendingCount() {
  try {
    const res = await fetch('/api/organize/stats')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    pendingCount.value = data.pending_review || 0
  } catch {
    // Fallback: try the old stats endpoint
    try {
      const res = await fetch('/api/stats')
      if (res.ok) {
        // Old API doesn't have pending count
        pendingCount.value = 0
      }
    } catch {
      // ignore
    }
  }
}

// --- Scanning ---
const isScanning = ref(false)
const scanProgress = ref(0)
const libraryPath = ref(localStorage.getItem('phos_library_path') || '/mnt/photos')
const scanMessage = ref('')
const scanError = ref('')

// Reference to the current route component for refreshing
const routeComponentRef = ref(null)

const startScan = async () => {
  if (isScanning.value) return
  isScanning.value = true
  scanMessage.value = ''
  scanError.value = ''
  scanProgress.value = 0

  try {
    const response = await fetch('/api/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: libraryPath.value })
    })

    if (!response.ok) {
      throw new Error(`Scan request failed: HTTP ${response.status}`)
    }

    const result = await response.json()
    scanMessage.value = 'Scan started successfully. Processing in background...'

    // Simulate progress while scan runs in background on server
    let progress = 0
    const interval = setInterval(() => {
      progress += 2
      scanProgress.value = Math.min(progress, 95)
      if (progress >= 95) {
        clearInterval(interval)
      }
    }, 300)

    // Poll to detect when scan finishes
    const pollInterval = setInterval(async () => {
      try {
        await fetchPendingCount()
        if (progress >= 95) {
          clearInterval(pollInterval)
          clearInterval(interval)
          scanProgress.value = 100
          scanMessage.value = 'Scan complete!'

          // Refresh the current route component's data
          await nextTick()
          const comp = routeComponentRef.value
          if (comp) {
            comp.loadData?.()
            comp.fetchPhotos?.()
            comp.fetchPeople?.()
            comp.fetchShots?.()
          }

          setTimeout(() => {
            isScanning.value = false
            scanProgress.value = 0
            scanMessage.value = ''
          }, 2000)
        }
      } catch {
        // ignore polling errors
      }
    }, 3000)

  } catch (e) {
    console.error('Scan failed:', e)
    scanError.value = e.message || 'Scan failed. Is the backend running?'

    let progress = 0
    const interval = setInterval(() => {
      progress += 5
      scanProgress.value = progress
      if (progress >= 100) {
        clearInterval(interval)
        isScanning.value = false
        scanProgress.value = 0
      }
    }, 200)
  }
}

// --- Import Dialog ---
const isDragging = ref(false)
const importPath = ref('')
const importMessage = ref('')
const importError = ref('')
const isImporting = ref(false)
const showImportDialog = ref(false)

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
      body: JSON.stringify({ path: pathToScan })
    })

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`)
    }

    importMessage.value = `Scanning "${pathToScan}" started. This may take a while...`

    setTimeout(async () => {
      await fetchPendingCount()
      const comp = routeComponentRef.value
      if (comp) {
        comp.loadData?.()
        comp.fetchPhotos?.()
        comp.fetchPeople?.()
        comp.fetchShots?.()
      }
      isImporting.value = false
    }, 5000)

  } catch (e) {
    console.error('Import scan failed:', e)
    importError.value = e.message || 'Failed to start scan. Is the backend running?'
    isImporting.value = false
  }
}

const uploadProgress = ref({ current: 0, total: 0 })
const isUploading = ref(false)

const handleDrop = async (e) => {
  isDragging.value = false
  importMessage.value = ''
  importError.value = ''

  const files = Array.from(e.dataTransfer?.files || [])
  const mediaFiles = files.filter(f => /\.(jpe?g|png|webp|mp4|mov|mkv|avi|webm)$/i.test(f.name))
  if (mediaFiles.length === 0) {
    importError.value = 'No supported media files found. Supported: JPEG, PNG, WebP, MP4, MOV, MKV, AVI, WebM.'
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
  if (failed === 0) {
    importMessage.value = `Uploaded ${succeeded} file${succeeded === 1 ? '' : 's'} successfully.`
  } else {
    importMessage.value = `Uploaded ${succeeded} of ${mediaFiles.length} files.`
    importError.value = `${failed} file${failed === 1 ? '' : 's'} failed to upload.`
  }

  // Refresh UI
  await fetchPendingCount()
  const comp = routeComponentRef.value
  if (comp) {
    comp.loadData?.()
    comp.fetchPhotos?.()
    comp.fetchPeople?.()
    comp.fetchShots?.()
  }
}

// --- Settings: Save library path ---
const settingsSaved = ref(false)

function saveLibraryPath() {
  localStorage.setItem('phos_library_path', libraryPath.value)
  settingsSaved.value = true
  setTimeout(() => { settingsSaved.value = false }, 2000)
}

// --- WebDAV Settings ---
const webdavEnabled = ref(false)
const webdavUsername = ref('')
const webdavPassword = ref('')
const webdavSaving = ref(false)
const webdavMessage = ref('')
const webdavError = ref('')
const webdavShowPassword = ref(false)
const webdavUrlCopied = ref(false)

const webdavUrl = computed(() => {
  return `${window.location.protocol}//${window.location.host}/webdav/`
})

async function fetchWebdavSettings() {
  try {
    const res = await fetch('/api/settings/webdav')
    if (!res.ok) return
    const data = await res.json()
    webdavEnabled.value = data.enabled
    webdavUsername.value = data.username || ''
  } catch {
    // ignore
  }
}

async function saveWebdavSettings() {
  if (!webdavPassword.value.trim()) {
    webdavError.value = 'Password is required.'
    return
  }

  webdavSaving.value = true
  webdavMessage.value = ''
  webdavError.value = ''

  try {
    const res = await fetch('/api/settings/webdav', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        password: webdavPassword.value.trim(),
      })
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    webdavMessage.value = 'WebDAV password saved!'
    webdavPassword.value = ''
    await fetchWebdavSettings()
    setTimeout(() => { webdavMessage.value = '' }, 3000)
  } catch (e) {
    webdavError.value = e.message || 'Failed to save password.'
  } finally {
    webdavSaving.value = false
  }
}

async function disableWebdav() {
  webdavMessage.value = ''
  webdavError.value = ''
  try {
    const res = await fetch('/api/settings/webdav', { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    webdavEnabled.value = false
    webdavPassword.value = ''
    webdavMessage.value = 'WebDAV disabled.'
    setTimeout(() => { webdavMessage.value = '' }, 3000)
  } catch (e) {
    webdavError.value = e.message || 'Failed to disable WebDAV.'
  }
}

function copyWebdavUrl() {
  navigator.clipboard.writeText(webdavUrl.value)
  webdavUrlCopied.value = true
  setTimeout(() => { webdavUrlCopied.value = false }, 2000)
}

// --- Init ---
onMounted(() => {
  fetchPendingCount()
  fetchWebdavSettings()
})
</script>

<template>
  <div class="min-h-screen bg-zinc-950 text-zinc-50 font-sans selection:bg-indigo-500/30">
    <!-- Navigation -->
    <header class="border-b border-white/5 bg-zinc-950/80 backdrop-blur-xl sticky top-0 z-50">
      <div class="max-w-7xl mx-auto px-4 sm:px-6 h-16 flex items-center justify-between">
        <div class="flex items-center gap-4 md:gap-8">
          <router-link to="/" class="flex items-center gap-2.5 group">
            <div class="w-9 h-9 rounded-xl flex items-center justify-center shadow-lg shadow-indigo-500/20 group-hover:scale-105 transition-transform overflow-hidden">
              <img src="/phos.svg" alt="Phos" class="w-9 h-9" />
            </div>
            <span class="text-xl font-bold tracking-tight text-white hidden xs:block">Phos</span>
          </router-link>

          <!-- Desktop Navigation -->
          <nav class="hidden md:flex items-center gap-1">
            <!-- Primary Nav -->
            <router-link to="/" custom v-slot="{ navigate }">
              <Button
                variant="ghost"
                :class="cn(
                  'gap-2 px-3 transition-colors',
                  currentView === 'organize'
                    ? 'text-white bg-white/10'
                    : 'text-zinc-400 hover:text-white hover:bg-white/5'
                )"
                @click="navigate"
              >
                <LayoutGrid class="w-4 h-4" />
                Organize
              </Button>
            </router-link>
            <router-link to="/review" custom v-slot="{ navigate }">
              <Button
                variant="ghost"
                :class="cn(
                  'gap-2 px-3 transition-colors',
                  currentView === 'review'
                    ? 'text-white bg-white/10'
                    : 'text-zinc-400 hover:text-white hover:bg-white/5'
                )"
                @click="navigate"
              >
                <ClipboardCheck class="w-4 h-4" />
                Review
                <span
                  v-if="pendingCount > 0"
                  class="ml-1 px-1.5 py-0.5 bg-yellow-500/20 text-yellow-400 rounded text-[10px] font-bold leading-none"
                >
                  {{ pendingCount }}
                </span>
              </Button>
            </router-link>
            <router-link to="/variations" custom v-slot="{ navigate }">
              <Button
                variant="ghost"
                :class="cn(
                  'gap-2 px-3 transition-colors',
                  currentView === 'variations'
                    ? 'text-white bg-white/10'
                    : 'text-zinc-400 hover:text-white hover:bg-white/5'
                )"
                @click="navigate"
              >
                <Layers class="w-4 h-4" />
                Variations
              </Button>
            </router-link>
            <router-link to="/people" custom v-slot="{ navigate }">
              <Button
                variant="ghost"
                :class="cn(
                  'gap-2 px-3 transition-colors',
                  currentView === 'people'
                    ? 'text-white bg-white/10'
                    : 'text-zinc-400 hover:text-white hover:bg-white/5'
                )"
                @click="navigate"
              >
                <Users class="w-4 h-4" />
                People
              </Button>
            </router-link>

            <!-- Separator -->
            <div class="h-5 w-[1px] bg-white/10 mx-2"></div>

            <!-- Secondary Nav -->
            <router-link to="/workflows" custom v-slot="{ navigate }">
              <Button
                variant="ghost"
                :class="cn(
                  'gap-2 px-3 transition-colors text-sm',
                  currentView === 'workflows'
                    ? 'text-white bg-white/10'
                    : 'text-zinc-500 hover:text-zinc-300 hover:bg-white/5'
                )"
                @click="navigate"
              >
                <Wand2 class="w-3.5 h-3.5" />
                Workflows
              </Button>
            </router-link>
          </nav>
        </div>

        <div class="flex items-center gap-2">
          <div class="relative hidden lg:block mr-2">
            <Search class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-zinc-500" />
            <Input
              placeholder="Search memories..."
              class="rounded-full pl-9 pr-4 py-1.5 h-9 w-64 bg-zinc-900/50 border-white/5 focus-visible:ring-indigo-500/40"
            />
          </div>

          <!-- Settings Sheet -->
          <Sheet>
            <SheetTrigger as-child>
              <Button variant="ghost" size="icon" class="text-zinc-400 hover:text-white rounded-xl hover:bg-white/5">
                <Settings class="w-5 h-5" />
              </Button>
            </SheetTrigger>
            <SheetContent>
              <SheetHeader class="mb-8 text-left">
                <SheetTitle>Settings</SheetTitle>
                <SheetDescription>Configure your personal photo laboratory.</SheetDescription>
              </SheetHeader>

              <div class="space-y-6">
                  <div class="space-y-2">
                    <Label>Library Path</Label>
                    <div class="flex gap-2">
                      <Input v-model="libraryPath" class="flex-1" placeholder="/path/to/photos" />
                      <Button
                        variant="outline"
                        size="icon"
                        class="shrink-0 border-white/10"
                        @click="saveLibraryPath"
                        :title="settingsSaved ? 'Saved!' : 'Save library path'"
                      >
                        <Check v-if="settingsSaved" class="w-4 h-4 text-emerald-500" />
                        <FolderOpen v-else class="w-4 h-4" />
                      </Button>
                    </div>
                    <p class="text-xs text-zinc-500">Enter the server-side directory path where your photos are stored.</p>
                    <div v-if="settingsSaved" class="text-xs text-emerald-500 font-medium">Path saved!</div>
                  </div>

                  <div class="space-y-4 pt-4">
                    <div class="flex items-center justify-between p-4 rounded-2xl bg-zinc-900/50 border border-white/5">
                      <div class="space-y-0.5">
                        <p class="text-sm font-medium text-white">Auto-Scan</p>
                        <p class="text-xs text-zinc-500">Watch for new files automatically</p>
                      </div>
                      <div class="w-10 h-5 bg-indigo-600 rounded-full relative">
                         <div class="absolute right-1 top-1 w-3 h-3 bg-white rounded-full"></div>
                      </div>
                    </div>
                  </div>

                  <!-- WebDAV Access -->
                  <div class="space-y-3 pt-6 border-t border-white/5">
                    <div class="flex items-center gap-2">
                      <Globe class="w-4 h-4 text-zinc-400" />
                      <Label class="text-sm font-medium">WebDAV Access</Label>
                      <span
                        v-if="webdavEnabled"
                        class="ml-auto px-2 py-0.5 bg-emerald-500/15 text-emerald-400 rounded text-[10px] font-bold"
                      >ENABLED</span>
                      <span
                        v-else
                        class="ml-auto px-2 py-0.5 bg-zinc-700/50 text-zinc-500 rounded text-[10px] font-bold"
                      >DISABLED</span>
                    </div>
                    <p class="text-xs text-zinc-500">
                      Mount your library as a read-only network drive via WebDAV.
                    </p>

                    <div class="space-y-2">
                      <Label class="text-xs text-zinc-400">Username</Label>
                      <div class="h-8 px-3 flex items-center text-sm text-zinc-300 bg-zinc-800/50 border border-white/10 rounded-md font-mono">
                        {{ webdavUsername }}
                      </div>
                    </div>
                    <div class="space-y-2">
                      <Label class="text-xs text-zinc-400">Password</Label>
                      <div class="flex gap-2">
                        <div class="relative flex-1">
                          <Input
                            v-model="webdavPassword"
                            :type="webdavShowPassword ? 'text' : 'password'"
                            :placeholder="webdavEnabled ? '(unchanged)' : 'Set a password'"
                            class="h-8 text-sm pr-9"
                          />
                          <button
                            type="button"
                            class="absolute right-2 top-1/2 -translate-y-1/2 text-zinc-500 hover:text-zinc-300"
                            @click="webdavShowPassword = !webdavShowPassword"
                          >
                            <EyeOff v-if="webdavShowPassword" class="w-3.5 h-3.5" />
                            <Eye v-else class="w-3.5 h-3.5" />
                          </button>
                        </div>
                      </div>
                    </div>

                    <div class="flex gap-2 pt-1">
                      <Button
                        size="sm"
                        class="bg-indigo-600 hover:bg-indigo-500 text-white text-xs"
                        :disabled="webdavSaving"
                        @click="saveWebdavSettings"
                      >
                        <RefreshCw v-if="webdavSaving" class="w-3 h-3 mr-1 animate-spin" />
                        {{ webdavEnabled ? 'Update' : 'Enable' }}
                      </Button>
                      <Button
                        v-if="webdavEnabled"
                        size="sm"
                        variant="outline"
                        class="text-xs border-red-500/30 text-red-400 hover:bg-red-500/10"
                        @click="disableWebdav"
                      >
                        <Trash2 class="w-3 h-3 mr-1" />
                        Disable
                      </Button>
                    </div>

                    <!-- Connection URL -->
                    <div v-if="webdavEnabled" class="p-3 rounded-xl bg-zinc-900/50 border border-white/5 space-y-1">
                      <p class="text-[10px] text-zinc-500 uppercase font-bold tracking-wider">Connection URL</p>
                      <div class="flex items-center gap-2">
                        <code class="text-xs text-indigo-400 flex-1 truncate">{{ webdavUrl }}</code>
                        <button
                          class="text-zinc-500 hover:text-white transition-colors shrink-0"
                          @click="copyWebdavUrl"
                          :title="webdavUrlCopied ? 'Copied!' : 'Copy URL'"
                        >
                          <Check v-if="webdavUrlCopied" class="w-3.5 h-3.5 text-emerald-500" />
                          <Copy v-else class="w-3.5 h-3.5" />
                        </button>
                      </div>
                    </div>

                    <!-- Feedback -->
                    <div v-if="webdavMessage" class="flex items-start gap-2 p-2 rounded-lg bg-emerald-500/10 border border-emerald-500/20">
                      <Check class="w-3.5 h-3.5 text-emerald-500 mt-0.5 shrink-0" />
                      <p class="text-xs text-emerald-400">{{ webdavMessage }}</p>
                    </div>
                    <div v-if="webdavError" class="flex items-start gap-2 p-2 rounded-lg bg-red-500/10 border border-red-500/20">
                      <AlertCircle class="w-3.5 h-3.5 text-red-500 mt-0.5 shrink-0" />
                      <p class="text-xs text-red-400">{{ webdavError }}</p>
                    </div>
                  </div>
              </div>
            </SheetContent>
          </Sheet>

          <!-- User menu / Logout -->
          <template v-if="authEnabled && user">
            <div class="flex items-center gap-2 text-sm text-zinc-400">
              <div class="w-7 h-7 rounded-full bg-indigo-600/30 border border-indigo-500/40 flex items-center justify-center text-xs font-bold text-indigo-300 uppercase shrink-0"
                   :title="userDisplayName">
                {{ userDisplayName.charAt(0) }}
              </div>
              <span class="hidden sm:inline max-w-[120px] truncate">{{ userDisplayName }}</span>
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="text-zinc-400 hover:text-white rounded-xl hover:bg-white/5"
              title="Sign out"
              @click="logout"
            >
              <LogOut class="w-4 h-4" />
            </Button>
          </template>

          <div class="h-6 w-[1px] bg-white/10 mx-1 hidden xs:block"></div>

          <!-- Import Dialog -->
          <Dialog v-model:open="showImportDialog">
            <DialogTrigger as-child>
              <Button class="bg-indigo-600 hover:bg-indigo-500 text-white shadow-lg shadow-indigo-500/20 rounded-xl gap-2 px-4 transition-all active:scale-95">
                <Upload class="w-4 h-4" />
                <span class="hidden sm:inline">Import</span>
              </Button>
            </DialogTrigger>
            <DialogContent class="sm:max-w-[425px]">
              <DialogHeader>
                <DialogTitle>Import Media</DialogTitle>
                <DialogDescription>
                  Enter a server-side directory path to scan for photos and videos.
                </DialogDescription>
              </DialogHeader>

              <!-- Server-side path input -->
              <div class="mt-4 space-y-4">
                <div class="space-y-2">
                  <Label>Directory Path</Label>
                  <div class="flex gap-2">
                    <Input
                      v-model="importPath"
                      :placeholder="libraryPath"
                      class="flex-1"
                    />
                    <Button
                      @click="handleImportScan"
                      :disabled="isImporting"
                      class="bg-indigo-600 hover:bg-indigo-500 text-white shrink-0"
                    >
                      <RefreshCw v-if="isImporting" class="w-4 h-4 mr-1 animate-spin" />
                      {{ isImporting ? 'Scanning...' : 'Scan' }}
                    </Button>
                  </div>
                  <p class="text-xs text-zinc-500">Leave empty to use the library path: {{ libraryPath }}</p>
                </div>

                <!-- Feedback messages -->
                <div v-if="importMessage" class="flex items-start gap-2 p-3 rounded-xl bg-emerald-500/10 border border-emerald-500/20">
                  <Check class="w-4 h-4 text-emerald-500 mt-0.5 shrink-0" />
                  <p class="text-sm text-emerald-400">{{ importMessage }}</p>
                </div>
                <div v-if="importError" class="flex items-start gap-2 p-3 rounded-xl bg-red-500/10 border border-red-500/20">
                  <AlertCircle class="w-4 h-4 text-red-500 mt-0.5 shrink-0" />
                  <p class="text-sm text-red-400">{{ importError }}</p>
                </div>
              </div>

              <!-- Drop zone for file upload -->
              <div
                @dragover.prevent="isDragging = true"
                @dragleave.prevent="isDragging = false"
                @drop.prevent="handleDrop"
                :class="cn(
                  'mt-4 flex flex-col items-center justify-center rounded-2xl border-2 border-dashed p-8 transition-all duration-300',
                  isDragging ? 'border-indigo-500 bg-indigo-500/10 scale-[0.98]' : 'border-white/10 bg-zinc-900/50 hover:border-white/20'
                )"
              >
                <template v-if="isUploading">
                  <RefreshCw class="w-6 h-6 text-indigo-400 animate-spin mb-3" />
                  <p class="text-sm font-medium text-white">Uploading {{ uploadProgress.current }} / {{ uploadProgress.total }}</p>
                </template>
                <template v-else>
                  <div class="w-12 h-12 rounded-full bg-zinc-800 flex items-center justify-center mb-4">
                    <Upload :class="cn('w-6 h-6 transition-colors', isDragging ? 'text-indigo-400' : 'text-zinc-500')" />
                  </div>
                  <p class="text-sm font-medium text-white">Drop files to upload</p>
                  <p class="text-xs text-zinc-500 mt-1 text-center">Drag photos or videos here to upload and index them.</p>
                </template>
              </div>
            </DialogContent>
          </Dialog>
        </div>
      </div>
    </header>

    <!-- Main Content Area -->
    <main class="max-w-7xl mx-auto p-4 sm:p-6 md:p-8 lg:p-10">
      <div class="rounded-[2rem] border border-white/5 bg-zinc-900/20 backdrop-blur-sm p-6 shadow-2xl">
        <router-view v-slot="{ Component }">
          <component :is="Component" ref="routeComponentRef" />
        </router-view>
      </div>
    </main>

    <!-- Footer Meta -->
    <footer class="mt-auto py-12 border-t border-white/5 text-center">
      <p class="text-[10px] text-zinc-600 font-bold tracking-[0.2em] uppercase">
        Phos {{ version }}
      </p>
    </footer>

    <!-- Mobile Navigation (Bottom) -->
    <div class="md:hidden fixed bottom-6 left-6 right-6 z-50">
       <div class="bg-zinc-900/90 backdrop-blur-xl border border-white/10 rounded-2xl p-2 flex items-center justify-around shadow-2xl shadow-black">
          <!-- Primary: Organize -->
          <router-link to="/" custom v-slot="{ navigate }">
            <Button
              variant="ghost"
              size="icon"
              :class="cn(
                'rounded-xl',
                currentView === 'organize' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
              )"
              @click="navigate"
            >
               <LayoutGrid class="w-5 h-5" />
            </Button>
          </router-link>
          <!-- Primary: Review -->
          <router-link to="/review" custom v-slot="{ navigate }">
            <Button
              variant="ghost"
              size="icon"
              :class="cn(
                'rounded-xl relative',
                currentView === 'review' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
              )"
              @click="navigate"
            >
               <ClipboardCheck class="w-5 h-5" />
               <span
                 v-if="pendingCount > 0"
                 class="absolute -top-1 -right-1 w-4 h-4 bg-yellow-500 rounded-full text-[9px] font-bold text-black flex items-center justify-center"
               >
                 {{ pendingCount > 9 ? '9+' : pendingCount }}
               </span>
            </Button>
          </router-link>
          <!-- Primary: People -->
          <router-link to="/people" custom v-slot="{ navigate }">
            <Button
              variant="ghost"
              size="icon"
              :class="cn(
                'rounded-xl',
                currentView === 'people' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
              )"
              @click="navigate"
            >
               <Users class="w-5 h-5" />
            </Button>
          </router-link>
          <!-- Secondary: Workflows -->
          <router-link to="/workflows" custom v-slot="{ navigate }">
            <Button
              variant="ghost"
              size="icon"
              :class="cn(
                'rounded-xl',
                currentView === 'workflows' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
              )"
              @click="navigate"
            >
               <Wand2 class="w-5 h-5" />
            </Button>
          </router-link>
       </div>
    </div>
  </div>
</template>

<style>
.text-glow {
  text-shadow: 0 0 30px rgba(99, 102, 241, 0.2);
}

/* Custom Scrollbar */
::-webkit-scrollbar {
  width: 8px;
}
::-webkit-scrollbar-track {
  background: transparent;
}
::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.05);
  border-radius: 10px;
}
::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.1);
}

@media (max-width: 640px) {
  .max-w-7xl {
    padding-left: 1rem;
    padding-right: 1rem;
  }
}
</style>

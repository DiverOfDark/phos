<script setup>
import { ref, onMounted, computed, nextTick } from 'vue'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { ScrollArea } from '@/components/ui/scroll-area'
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'

import Gallery from '@/components/Gallery.vue'
import PeopleList from '@/components/PeopleList.vue'
import Timeline from '@/components/Timeline.vue'

import {
  Activity,
  Image as ImageIcon,
  Users,
  Settings,
  Search,
  Upload,
  LayoutGrid,
  RefreshCw,
  Clock,
  HardDrive,
  Shield,
  Zap,
  FolderOpen,
  Check,
  AlertCircle
} from 'lucide-vue-next'

// --- Navigation ---
const currentView = ref('library')

function setView(view) {
  currentView.value = view
}

// --- View titles ---
const viewTitle = computed(() => {
  switch (currentView.value) {
    case 'library': return 'Library'
    case 'people': return 'People'
    case 'timeline': return 'Timeline'
    default: return 'Library'
  }
})

const viewDescription = computed(() => {
  switch (currentView.value) {
    case 'library': return 'Your personal AI-curated photo laboratory.'
    case 'people': return 'Face clusters detected by the AI engine.'
    case 'timeline': return 'Browse your memories chronologically.'
    default: return 'Your personal AI-curated photo laboratory.'
  }
})

// --- Stats ---
const stats = ref([
  { name: 'Total Media', value: '0', icon: ImageIcon, description: 'Images and videos indexed' },
  { name: 'Detected People', value: '0', icon: Users, description: 'Face clusters identified' },
  { name: 'System Status', value: 'Idle', icon: Activity, description: 'Backend processing state' },
])

async function fetchStats() {
  try {
    const res = await fetch('/api/stats')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    stats.value = [
      { name: 'Total Media', value: String(data.total_photos || 0), icon: ImageIcon, description: `${data.total_files || 0} files indexed` },
      { name: 'Detected People', value: String(data.total_people || 0), icon: Users, description: 'Face clusters identified' },
      { name: 'System Status', value: 'Online', icon: Activity, description: 'Backend connected' },
    ]
  } catch (e) {
    console.warn('Could not fetch stats:', e.message)
    stats.value[2] = { name: 'System Status', value: 'Offline', icon: Activity, description: 'Backend not reachable' }
  }
}

// --- Scanning ---
const isScanning = ref(false)
const scanProgress = ref(0)
const libraryPath = ref(localStorage.getItem('phos_library_path') || '/mnt/photos')
const scanMessage = ref('')
const scanError = ref('')

// Reference to Gallery/Timeline/People components for refreshing
const galleryRef = ref(null)
const peopleRef = ref(null)
const timelineRef = ref(null)

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

    // Poll stats to detect when scan finishes (stats change)
    const pollInterval = setInterval(async () => {
      try {
        await fetchStats()
        // After some time, assume scan is done and refresh
        if (progress >= 95) {
          clearInterval(pollInterval)
          clearInterval(interval)
          scanProgress.value = 100
          scanMessage.value = 'Scan complete!'

          // Refresh gallery, timeline, and people data
          await nextTick()
          if (galleryRef.value?.fetchPhotos) {
            galleryRef.value.fetchPhotos()
          }
          if (timelineRef.value?.fetchPhotos) {
            timelineRef.value.fetchPhotos()
          }
          if (peopleRef.value?.fetchPeople) {
            peopleRef.value.fetchPeople()
          }

          setTimeout(() => {
            isScanning.value = false
            scanProgress.value = 0
            scanMessage.value = ''
          }, 2000)
        }
      } catch (e) {
        // ignore polling errors
      }
    }, 3000)

  } catch (e) {
    console.error('Scan failed:', e)
    scanError.value = e.message || 'Scan failed. Is the backend running?'

    // Still show progress animation for visual feedback
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

    // Wait a bit then refresh stats, gallery, timeline, and people
    setTimeout(async () => {
      await fetchStats()
      if (galleryRef.value?.fetchPhotos) {
        galleryRef.value.fetchPhotos()
      }
      if (timelineRef.value?.fetchPhotos) {
        timelineRef.value.fetchPhotos()
      }
      if (peopleRef.value?.fetchPeople) {
        peopleRef.value.fetchPeople()
      }
      isImporting.value = false
    }, 5000)

  } catch (e) {
    console.error('Import scan failed:', e)
    importError.value = e.message || 'Failed to start scan. Is the backend running?'
    isImporting.value = false
  }
}

const handleDrop = (e) => {
  isDragging.value = false
  // Since this is a server-side app, drag and drop of local files doesn't apply.
  // Show a helpful message instead.
  importMessage.value = 'Phos scans server-side directories. Enter a path above and click "Scan Directory".'
}

// --- Settings: Save library path ---
const settingsSaved = ref(false)

function saveLibraryPath() {
  localStorage.setItem('phos_library_path', libraryPath.value)
  settingsSaved.value = true
  setTimeout(() => { settingsSaved.value = false }, 2000)
}

// --- Has content (to decide whether to show empty state or gallery) ---
const hasPhotos = computed(() => {
  const mediaVal = parseInt(stats.value[0]?.value)
  return !isNaN(mediaVal) && mediaVal > 0
})

// --- Init ---
onMounted(() => {
  fetchStats()
})
</script>

<template>
  <div class="min-h-screen bg-zinc-950 text-zinc-50 font-sans selection:bg-indigo-500/30">
    <!-- Navigation -->
    <header class="border-b border-white/5 bg-zinc-950/80 backdrop-blur-xl sticky top-0 z-50">
      <div class="max-w-7xl mx-auto px-4 sm:px-6 h-16 flex items-center justify-between">
        <div class="flex items-center gap-4 md:gap-8">
          <div class="flex items-center gap-2.5 group cursor-pointer" @click="setView('library')">
            <div class="w-9 h-9 bg-indigo-600 rounded-xl flex items-center justify-center shadow-lg shadow-indigo-500/20 group-hover:scale-105 transition-transform">
              <span class="text-white font-black text-lg">P</span>
            </div>
            <span class="text-xl font-bold tracking-tight text-white hidden xs:block">Phos</span>
          </div>

          <nav class="hidden md:flex items-center gap-1">
            <Button
              variant="ghost"
              :class="cn(
                'gap-2 px-3 transition-colors',
                currentView === 'library'
                  ? 'text-white bg-white/10'
                  : 'text-zinc-400 hover:text-white hover:bg-white/5'
              )"
              @click="setView('library')"
            >
              <LayoutGrid class="w-4 h-4" />
              Library
            </Button>
            <Button
              variant="ghost"
              :class="cn(
                'gap-2 px-3 transition-colors',
                currentView === 'people'
                  ? 'text-white bg-white/10'
                  : 'text-zinc-400 hover:text-white hover:bg-white/5'
              )"
              @click="setView('people')"
            >
              <Users class="w-4 h-4" />
              People
            </Button>
            <Button
              variant="ghost"
              :class="cn(
                'gap-2 px-3 transition-colors',
                currentView === 'timeline'
                  ? 'text-white bg-white/10'
                  : 'text-zinc-400 hover:text-white hover:bg-white/5'
              )"
              @click="setView('timeline')"
            >
              <Clock class="w-4 h-4" />
              Timeline
            </Button>
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

              <Tabs default-value="general" class="w-full">
                <TabsList class="grid w-full grid-cols-2 mb-6">
                  <TabsTrigger value="general">General</TabsTrigger>
                  <TabsTrigger value="ai">AI Models</TabsTrigger>
                </TabsList>

                <TabsContent value="general" class="space-y-6">
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
                </TabsContent>

                <TabsContent value="ai" class="space-y-4 text-center py-12">
                   <Zap class="w-12 h-12 text-indigo-500 mx-auto mb-4 opacity-20" />
                   <p class="text-white font-medium">Neural Engine v1.2</p>
                   <p class="text-sm text-zinc-500">Face detection and object recognition models are currently managed by the system.</p>
                </TabsContent>
              </Tabs>

              <div class="absolute bottom-8 left-6 right-6">
                 <div class="p-4 rounded-2xl bg-zinc-900 border border-white/5 flex items-center gap-3">
                    <Shield class="w-5 h-5 text-emerald-500" />
                    <div>
                       <p class="text-xs font-bold text-white uppercase tracking-wider">Privacy Mode</p>
                       <p class="text-[10px] text-zinc-500">All processing is strictly local.</p>
                    </div>
                 </div>
              </div>
            </SheetContent>
          </Sheet>

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

              <!-- Drop zone (still present but with guidance) -->
              <div
                @dragover.prevent="isDragging = true"
                @dragleave.prevent="isDragging = false"
                @drop.prevent="handleDrop"
                :class="cn(
                  'mt-4 flex flex-col items-center justify-center rounded-2xl border-2 border-dashed p-8 transition-all duration-300',
                  isDragging ? 'border-indigo-500 bg-indigo-500/10 scale-[0.98]' : 'border-white/10 bg-zinc-900/50 hover:border-white/20'
                )"
              >
                <div class="w-12 h-12 rounded-full bg-zinc-800 flex items-center justify-center mb-4">
                  <HardDrive :class="cn('w-6 h-6 transition-colors', isDragging ? 'text-indigo-400' : 'text-zinc-500')" />
                </div>
                <p class="text-sm font-medium text-white">Server-side scanning</p>
                <p class="text-xs text-zinc-500 mt-1 text-center">Phos scans directories on the server where it runs. Enter a path above to begin.</p>
              </div>
            </DialogContent>
          </Dialog>
        </div>
      </div>
    </header>

    <main class="max-w-7xl mx-auto p-4 sm:p-6 md:p-8 lg:p-10">
      <!-- Welcome Header -->
      <div class="mb-8 md:mb-12">
        <h2 class="text-3xl md:text-4xl font-bold tracking-tight text-white mb-3 text-glow">{{ viewTitle }}</h2>
        <p class="text-zinc-400 text-lg">{{ viewDescription }}</p>
      </div>

      <!-- Stats Grid -->
      <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 md:gap-6 mb-12">
        <Card v-for="stat in stats" :key="stat.name" class="bg-zinc-900/40 border-white/5 backdrop-blur-sm group hover:border-indigo-500/30 transition-all duration-300">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-semibold text-zinc-400 group-hover:text-zinc-300">{{ stat.name }}</CardTitle>
            <component :is="stat.icon" class="w-4 h-4 text-zinc-500 group-hover:text-indigo-400 transition-colors" />
          </CardHeader>
          <CardContent>
            <div class="text-3xl font-bold text-white tracking-tight">{{ stat.value }}</div>
            <p class="text-xs text-zinc-500 mt-2 font-medium leading-relaxed">{{ stat.description }}</p>
          </CardContent>
        </Card>
      </div>

      <!-- Main Content Area -->
      <div class="relative">
        <div class="flex items-center justify-between mb-6">
          <h3 class="text-lg font-semibold text-white/90">
            {{ currentView === 'people' ? 'Detected Faces' : currentView === 'timeline' ? 'Your Timeline' : 'Recent Discovery' }}
          </h3>
          <Button
            v-if="currentView !== 'people' && currentView !== 'timeline'"
            variant="link"
            class="text-indigo-400 hover:text-indigo-300 p-0 h-auto font-medium"
            @click="setView('library')"
          >
            Browse all
          </Button>
        </div>

        <!-- Library View -->
        <div v-if="currentView === 'library'">
          <div v-if="hasPhotos" class="rounded-[2rem] border border-white/5 bg-zinc-900/20 backdrop-blur-sm p-6 shadow-2xl">
            <Gallery ref="galleryRef" />
          </div>
          <ScrollArea v-else class="h-[400px] md:h-[500px] w-full rounded-[2rem] border border-white/5 bg-zinc-900/20 backdrop-blur-sm relative overflow-hidden group shadow-2xl">
            <!-- Background Decoration -->
            <div class="absolute inset-0 bg-gradient-to-br from-indigo-500/5 via-transparent to-purple-500/5 opacity-0 group-hover:opacity-100 transition-opacity duration-700 pointer-events-none"></div>

            <div class="flex flex-col items-center justify-center h-full text-center p-8 sm:p-12 space-y-6 relative z-10">
              <div class="relative">
                <div class="absolute inset-0 bg-indigo-500 blur-3xl opacity-10 animate-pulse"></div>
                <div class="w-20 h-20 bg-zinc-900 rounded-2xl flex items-center justify-center border border-white/5 shadow-2xl relative transition-transform group-hover:scale-110 duration-500">
                  <ImageIcon class="w-10 h-10 text-zinc-700 group-hover:text-indigo-500 transition-colors duration-500" />
                </div>
              </div>

              <div class="max-w-xs mx-auto">
                <p class="text-xl font-bold text-white mb-2">No memories found</p>
                <p class="text-zinc-500 text-sm leading-relaxed">
                  Connect your library folder to start the AI-powered indexing and face clustering process.
                </p>
              </div>

              <div class="w-full max-w-xs space-y-4">
                 <Button
                  @click="startScan"
                  :disabled="isScanning"
                  class="w-full bg-white text-black hover:bg-zinc-200 font-bold px-8 py-6 rounded-2xl transition-all active:scale-95 disabled:opacity-50 h-auto shadow-xl"
                >
                  <RefreshCw v-if="isScanning" class="w-5 h-5 mr-2 animate-spin" />
                  {{ isScanning ? 'Initializing Engine...' : 'Scan Library' }}
                </Button>

                <!-- Scan feedback messages -->
                <div v-if="scanMessage" class="flex items-start gap-2 p-3 rounded-xl bg-emerald-500/10 border border-emerald-500/20">
                  <Check class="w-4 h-4 text-emerald-500 mt-0.5 shrink-0" />
                  <p class="text-xs text-emerald-400">{{ scanMessage }}</p>
                </div>
                <div v-if="scanError" class="flex items-start gap-2 p-3 rounded-xl bg-red-500/10 border border-red-500/20">
                  <AlertCircle class="w-4 h-4 text-red-500 mt-0.5 shrink-0" />
                  <p class="text-xs text-red-400">{{ scanError }}</p>
                </div>

                <div v-if="isScanning" class="w-full bg-zinc-800 h-1.5 rounded-full overflow-hidden">
                   <div class="bg-indigo-500 h-full transition-all duration-300" :style="{ width: `${scanProgress}%` }"></div>
                </div>
              </div>
            </div>
          </ScrollArea>
        </div>

        <!-- Timeline View -->
        <div v-if="currentView === 'timeline'" class="rounded-[2rem] border border-white/5 bg-zinc-900/20 backdrop-blur-sm p-6 shadow-2xl">
          <Timeline ref="timelineRef" />
        </div>

        <!-- People View -->
        <div v-if="currentView === 'people'" class="rounded-[2rem] border border-white/5 bg-zinc-900/20 backdrop-blur-sm p-6 shadow-2xl">
          <PeopleList ref="peopleRef" />
        </div>
      </div>
    </main>

    <!-- Footer Meta -->
    <footer class="mt-auto py-12 border-t border-white/5 text-center">
      <div class="flex items-center justify-center gap-2 mb-4 opacity-40">
        <HardDrive class="w-3 h-3" />
        <span class="text-[10px] font-bold tracking-widest uppercase">Local Environment</span>
      </div>
      <p class="text-[10px] text-zinc-600 font-bold tracking-[0.2em] uppercase">
        Phos v0.1.0-alpha &bull; Precision &bull; Privacy
      </p>
    </footer>

    <!-- Mobile Navigation (Bottom) -->
    <div class="md:hidden fixed bottom-6 left-6 right-6 z-50">
       <div class="bg-zinc-900/90 backdrop-blur-xl border border-white/10 rounded-2xl p-2 flex items-center justify-around shadow-2xl shadow-black">
          <Button
            variant="ghost"
            size="icon"
            :class="cn(
              'rounded-xl',
              currentView === 'library' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
            )"
            @click="setView('library')"
          >
             <LayoutGrid class="w-5 h-5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            :class="cn(
              'rounded-xl',
              currentView === 'people' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
            )"
            @click="setView('people')"
          >
             <Users class="w-5 h-5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            :class="cn(
              'rounded-xl',
              currentView === 'timeline' ? 'text-indigo-500 bg-white/5' : 'text-zinc-400'
            )"
            @click="setView('timeline')"
          >
             <Clock class="w-5 h-5" />
          </Button>
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

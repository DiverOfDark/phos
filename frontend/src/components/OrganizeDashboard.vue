<script setup>
import { ref, computed, onMounted, defineExpose } from 'vue'
import { useRouter } from 'vue-router'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import PersonNamer from '@/components/PersonNamer.vue'
import {
  Image as ImageIcon,
  Users,
  ClipboardCheck,
  AlertTriangle,
  FolderOpen,
  RefreshCw,
  ArrowRight,
  UserPlus,
  ScanLine,
  Check,
  AlertCircle,
} from 'lucide-vue-next'

const router = useRouter()

const stats = ref({
  total_shots: 0,
  total_files: 0,
  total_people: 0,
  pending_review: 0,
  confirmed: 0,
  unsorted: 0,
  unnamed_people: 0,
})
const people = ref([])
const loading = ref(true)
const error = ref(null)

// Scan state
const isScanning = ref(false)
const scanProgress = ref(0)
const scanMessage = ref('')
const scanError = ref('')
const libraryPath = ref(localStorage.getItem('phos_library_path') || '/mnt/photos')

// PersonNamer dialog
const showNamer = ref(false)

const progressPercent = computed(() => {
  const total = stats.value.total_shots
  if (total === 0) return 0
  return Math.round((stats.value.confirmed / total) * 100)
})

async function fetchStats() {
  try {
    const res = await fetch('/api/organize/stats')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    stats.value = await res.json()
  } catch (e) {
    // Fallback: try the old stats endpoint
    try {
      const res = await fetch('/api/stats')
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const data = await res.json()
      stats.value = {
        total_shots: data.total_shots || 0,
        total_files: data.total_files || 0,
        total_people: data.total_people || 0,
        pending_review: 0,
        confirmed: 0,
        unsorted: 0,
        unnamed_people: 0,
      }
    } catch {
      // ignore
    }
  }
}

async function fetchPeople() {
  try {
    const res = await fetch('/api/people')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    people.value = await res.json()
  } catch (e) {
    console.warn('Could not fetch people:', e.message)
  }
}

async function loadData() {
  loading.value = true
  error.value = null
  try {
    await Promise.all([fetchStats(), fetchPeople()])
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

async function startScan() {
  if (isScanning.value) return
  isScanning.value = true
  scanMessage.value = ''
  scanError.value = ''
  scanProgress.value = 0

  try {
    const response = await fetch('/api/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: libraryPath.value }),
    })

    if (!response.ok) {
      throw new Error(`Scan request failed: HTTP ${response.status}`)
    }

    scanMessage.value = 'Scan started. Processing in background...'

    // Simulate progress while scan runs in background
    let progress = 0
    const interval = setInterval(() => {
      progress += 2
      scanProgress.value = Math.min(progress, 95)
      if (progress >= 95) {
        clearInterval(interval)
      }
    }, 300)

    // Poll stats to detect completion
    const pollInterval = setInterval(async () => {
      try {
        await fetchStats()
        await fetchPeople()
        if (progress >= 95) {
          clearInterval(pollInterval)
          clearInterval(interval)
          scanProgress.value = 100
          scanMessage.value = 'Scan complete!'

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

function onNamerChanged() {
  fetchPeople()
  fetchStats()
}

onMounted(loadData)

defineExpose({ loadData, fetchPeople })
</script>

<template>
  <div>
    <div class="mb-8">
      <h2 class="text-2xl font-bold text-white mb-2">Organization Overview</h2>
      <p class="text-zinc-400 text-sm">Track your photo organization progress and take action.</p>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="flex items-center justify-center py-20">
      <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
    </div>

    <div v-else>
      <!-- Overall progress bar -->
      <div v-if="stats.total_shots > 0" class="mb-6">
        <div class="flex items-center justify-between mb-2">
          <span class="text-sm font-medium text-zinc-300">Organization Progress</span>
          <span class="text-sm font-medium text-zinc-400">{{ progressPercent }}%</span>
        </div>
        <div class="w-full bg-zinc-800 h-2 rounded-full overflow-hidden">
          <div
            class="bg-emerald-500 h-full rounded-full transition-all duration-500"
            :style="{ width: `${progressPercent}%` }"
          ></div>
        </div>
        <p class="text-xs text-zinc-500 mt-1">
          {{ stats.confirmed }} of {{ stats.total_shots }} shots confirmed
        </p>
      </div>

      <!-- Stats grid -->
      <div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
        <Card class="bg-zinc-900/40 border-white/5">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-semibold text-zinc-400">Pending</CardTitle>
            <AlertTriangle class="w-4 h-4 text-yellow-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold text-white">{{ stats.pending_review }}</div>
            <p class="text-xs text-zinc-500 mt-1">shots awaiting review</p>
          </CardContent>
        </Card>

        <Card class="bg-zinc-900/40 border-white/5">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-semibold text-zinc-400">Confirmed</CardTitle>
            <ClipboardCheck class="w-4 h-4 text-emerald-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold text-white">{{ stats.confirmed }}</div>
            <p class="text-xs text-zinc-500 mt-1">shots organized</p>
          </CardContent>
        </Card>

        <Card class="bg-zinc-900/40 border-white/5">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-semibold text-zinc-400">People</CardTitle>
            <Users class="w-4 h-4 text-indigo-400" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold text-white">{{ stats.total_people }}</div>
            <p v-if="stats.unnamed_people > 0" class="text-xs text-yellow-500 mt-1">{{ stats.unnamed_people }} unnamed</p>
            <p v-else class="text-xs text-zinc-500 mt-1">all named</p>
          </CardContent>
        </Card>

        <Card class="bg-zinc-900/40 border-white/5">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-semibold text-zinc-400">Unsorted</CardTitle>
            <FolderOpen class="w-4 h-4 text-zinc-400" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold text-white">{{ stats.unsorted }}</div>
            <p class="text-xs text-zinc-500 mt-1">shots without a person</p>
          </CardContent>
        </Card>
      </div>

      <!-- Quick actions -->
      <div class="flex flex-wrap gap-3 mb-8">
        <Button
          v-if="stats.pending_review > 0"
          class="bg-indigo-600 hover:bg-indigo-500 text-white gap-2"
          @click="router.push('/review')"
        >
          <ClipboardCheck class="w-4 h-4" />
          Review Pending
          <span class="ml-1 px-1.5 py-0.5 bg-white/20 rounded text-xs">{{ stats.pending_review }}</span>
        </Button>
        <Button
          v-if="stats.unnamed_people > 0"
          class="bg-indigo-600 hover:bg-indigo-500 text-white gap-2"
          @click="showNamer = true"
        >
          <UserPlus class="w-4 h-4" />
          Name People
          <span class="ml-1 px-1.5 py-0.5 bg-white/20 rounded text-xs">{{ stats.unnamed_people }}</span>
        </Button>
        <Button
          v-if="stats.unsorted > 0"
          variant="outline"
          class="border-white/10 text-zinc-300 hover:text-white gap-2"
          @click="router.push('/review?status=unsorted')"
        >
          <FolderOpen class="w-4 h-4" />
          View Unsorted
        </Button>
      </div>

      <!-- Scan section -->
      <div class="mb-8 p-4 rounded-xl bg-zinc-900/40 border border-white/5">
        <div class="flex items-center justify-between mb-3">
          <div>
            <h3 class="text-sm font-semibold text-white">Library Scan</h3>
            <p class="text-xs text-zinc-500 mt-0.5">Scan your library to detect new photos and faces.</p>
          </div>
          <Button
            :disabled="isScanning"
            class="bg-indigo-600 hover:bg-indigo-500 text-white gap-2"
            @click="startScan"
          >
            <RefreshCw v-if="isScanning" class="w-4 h-4 animate-spin" />
            <ScanLine v-else class="w-4 h-4" />
            {{ isScanning ? 'Scanning...' : 'Scan Library' }}
          </Button>
        </div>

        <div class="flex gap-2 mb-3">
          <Input
            v-model="libraryPath"
            placeholder="/path/to/photos"
            class="flex-1 h-8 text-sm bg-zinc-800/50 border-white/5"
            :disabled="isScanning"
          />
        </div>

        <!-- Scan progress bar -->
        <div v-if="isScanning" class="mb-2">
          <div class="w-full bg-zinc-800 h-1.5 rounded-full overflow-hidden">
            <div
              class="bg-indigo-500 h-full rounded-full transition-all duration-300"
              :style="{ width: `${scanProgress}%` }"
            ></div>
          </div>
        </div>

        <!-- Scan feedback -->
        <div v-if="scanMessage" class="flex items-start gap-2 p-2 rounded-lg bg-emerald-500/10 border border-emerald-500/20">
          <Check class="w-3.5 h-3.5 text-emerald-500 mt-0.5 shrink-0" />
          <p class="text-xs text-emerald-400">{{ scanMessage }}</p>
        </div>
        <div v-if="scanError" class="flex items-start gap-2 p-2 rounded-lg bg-red-500/10 border border-red-500/20">
          <AlertCircle class="w-3.5 h-3.5 text-red-500 mt-0.5 shrink-0" />
          <p class="text-xs text-red-400">{{ scanError }}</p>
        </div>
      </div>

      <!-- People summary -->
      <div v-if="people.length > 0">
        <div class="flex items-center justify-between mb-4">
          <h3 class="text-lg font-semibold text-white">People</h3>
          <router-link to="/people" class="text-indigo-400 hover:text-indigo-300 text-sm font-medium flex items-center gap-1">
            View all <ArrowRight class="w-3.5 h-3.5" />
          </router-link>
        </div>
        <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-4">
          <div
            v-for="person in people.slice(0, 12)"
            :key="person.id"
            class="flex flex-col items-center p-3 rounded-xl bg-zinc-900/40 border border-white/5 hover:border-indigo-500/50 group cursor-pointer transition-colors"
            @click="router.push({ name: 'person-detail', params: { id: person.id } })"
          >
            <div class="w-16 h-16 rounded-full bg-zinc-800 border-2 border-zinc-700 group-hover:border-indigo-500 overflow-hidden flex items-center justify-center transition-colors mb-2">
              <img
                v-if="person.thumbnail_url"
                :src="person.thumbnail_url"
                class="w-full h-full object-cover"
              />
              <span v-else class="text-lg font-bold text-zinc-600">{{ (person.name || '?')[0] }}</span>
            </div>
            <span class="text-xs font-medium text-zinc-300 group-hover:text-white transition-colors truncate max-w-full">
              {{ person.name || 'Unnamed' }}
            </span>
            <div class="flex items-center gap-2 mt-1">
              <span class="text-[10px] text-zinc-500">{{ person.shot_count || 0 }} shots</span>
              <span
                v-if="(person.pending_count || 0) > 0"
                class="text-[10px] text-yellow-500"
              >
                {{ person.pending_count }} pending
              </span>
            </div>
          </div>
        </div>
      </div>

      <!-- Empty state -->
      <div v-else-if="stats.total_shots === 0" class="text-center py-16">
        <ImageIcon class="w-12 h-12 text-zinc-700 mx-auto mb-4" />
        <p class="text-white font-medium mb-2">No shots yet</p>
        <p class="text-zinc-500 text-sm mb-6">Scan a library to start organizing your photos.</p>
      </div>
    </div>

    <!-- PersonNamer dialog -->
    <PersonNamer
      v-model:open="showNamer"
      @changed="onNamerChanged"
    />
  </div>
</template>

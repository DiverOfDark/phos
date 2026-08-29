<script setup>
/**
 * Overview — the library as a route diagram.
 *
 * Files enter at the left and end up filed at the right; each station shows how
 * many are sitting there. Anything that needs a person is a work-queue row
 * underneath, counted and one click from the desk that clears it.
 */
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'

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

// --- Scan ---
const isScanning = ref(false)
const scanProgress = ref(0)
const scanMessage = ref('')
const scanError = ref('')
const libraryPath = ref(localStorage.getItem('phos_library_path') || '/mnt/photos')
const lastScan = ref(localStorage.getItem('phos_last_scan') || '')

const lastScanLabel = computed(() => {
  if (!lastScan.value) return 'never'
  const then = new Date(lastScan.value)
  if (Number.isNaN(then.getTime())) return 'never'
  const mins = Math.floor((Date.now() - then.getTime()) / 60000)
  if (mins < 1) return 'just now'
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  return `${Math.floor(hours / 24)}d ago`
})

const progressPercent = computed(() => {
  const total = stats.value.total_shots
  if (total === 0) return 0
  return Math.round((stats.value.confirmed / total) * 100)
})

/** The route diagram: one station per stage, tracks drawn between them. */
const stations = computed(() => {
  const s = stats.value
  const st = (label, count, sub, color) => ({ label, count, sub, color })
  return [
    st('FILES', s.total_files, 'on disk', s.total_files ? 'var(--status-ready)' : 'var(--status-stopped)'),
    st('SHOTS', s.total_shots, 'grouped', s.total_shots ? 'var(--status-ready)' : 'var(--status-stopped)'),
    st('PENDING', s.pending_review, 'awaiting review', s.pending_review ? 'var(--status-degraded)' : 'var(--status-ready)'),
    st('UNSORTED', s.unsorted, 'no person yet', s.unsorted ? 'var(--status-pending)' : 'var(--status-ready)'),
    st('FILED', s.confirmed, 'confirmed', s.confirmed ? 'var(--status-ready)' : 'var(--status-stopped)'),
  ]
})

const workRows = computed(() => {
  const s = stats.value
  const rows = []
  if (s.pending_review > 0) {
    rows.push({
      key: 'pending',
      count: s.pending_review,
      dot: 'var(--status-degraded)',
      title: 'shots pending review',
      sub: 'confirm or route each one',
      go: () => router.push('/review'),
    })
  }
  if (s.unnamed_people > 0) {
    rows.push({
      key: 'clusters',
      count: s.unnamed_people,
      dot: 'var(--status-degraded)',
      title: 'unnamed face clusters',
      sub: 'name or merge them',
      go: () => router.push({ path: '/review', query: { lane: 'faces' } }),
    })
  }
  if (s.unsorted > 0) {
    rows.push({
      key: 'unsorted',
      count: s.unsorted,
      dot: 'var(--status-pending)',
      title: 'shots without a person',
      sub: 'left unsorted on purpose or not yet seen',
      go: () => router.push({ path: '/review', query: { status: 'unsorted' } }),
    })
  }
  return rows
})

async function fetchStats() {
  try {
    const res = await fetch('/api/organize/stats')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    stats.value = await res.json()
  } catch {
    // Fallback: the older stats endpoint has no queue counts.
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
    } catch { /* the empty state covers it */ }
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
  try {
    await Promise.all([fetchStats(), fetchPeople()])
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
  localStorage.setItem('phos_library_path', libraryPath.value)

  try {
    const response = await fetch('/api/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: libraryPath.value }),
    })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)

    lastScan.value = new Date().toISOString()
    localStorage.setItem('phos_last_scan', lastScan.value)
    scanMessage.value = `walking ${libraryPath.value}`

    // The server scans in the background and reports no percentage, so the bar
    // creeps to 95% and the poll below decides when it is actually done.
    let progress = 0
    const interval = setInterval(() => {
      progress += 2
      scanProgress.value = Math.min(progress, 95)
      if (progress >= 95) clearInterval(interval)
    }, 300)

    const pollInterval = setInterval(async () => {
      try {
        await fetchStats()
        await fetchPeople()
        if (progress >= 95) {
          clearInterval(pollInterval)
          clearInterval(interval)
          scanProgress.value = 100
          scanMessage.value = 'scan complete'
          setTimeout(() => {
            isScanning.value = false
            scanProgress.value = 0
            scanMessage.value = ''
          }, 2000)
        }
      } catch { /* keep polling */ }
    }, 3000)
  } catch (e) {
    console.error('Scan failed:', e)
    scanError.value = e.message || 'scan request failed — is the backend running?'
    isScanning.value = false
    scanProgress.value = 0
  }
}

onMounted(loadData)

defineExpose({ loadData, fetchPeople })
</script>

<template>
  <div class="p-4 md:p-8 max-w-[1040px] w-full mx-auto flex flex-col gap-8">
    <div class="flex flex-wrap items-baseline justify-between gap-4">
      <h2 class="text-[22px] font-semibold">Library</h2>
      <div class="font-mono text-xs text-ink-tertiary">
        {{ stats.total_files }} files · {{ stats.total_shots }} shots · last scan {{ lastScanLabel }}
      </div>
    </div>

    <div v-if="loading" class="font-mono text-xs text-ink-tertiary py-16 text-center">loading library…</div>

    <template v-else>
      <!-- Pipeline route diagram -->
      <div class="card-ab p-6">
        <div class="label mb-6">Pipeline</div>
        <div class="flex items-start overflow-x-auto">
          <div
            v-for="(st, i) in stations"
            :key="st.label"
            class="flex items-start"
            :class="i === stations.length - 1 ? 'flex-none' : 'flex-1'"
          >
            <div class="flex flex-col gap-2 items-start min-w-[88px]">
              <div class="flex items-center h-4">
                <span
                  class="w-2.5 h-2.5 rounded-full"
                  :style="{ background: st.color, outline: `1px solid ${st.color}`, border: '2px solid var(--bg-surface)' }"
                ></span>
              </div>
              <div class="font-mono text-[11px] tracking-[0.08em] text-ink-tertiary">{{ st.label }}</div>
              <div class="font-mono text-lg font-medium" :style="{ color: st.count ? 'var(--text-primary)' : 'var(--text-tertiary)' }">
                {{ st.count }}
              </div>
              <div class="text-xs font-light text-ink-secondary whitespace-nowrap">{{ st.sub }}</div>
            </div>
            <div
              v-if="i < stations.length - 1"
              class="flex-1 h-0.5 mt-[7px] mx-4 min-w-6"
              :style="{ background: 'var(--border-strong)' }"
            ></div>
          </div>
        </div>
      </div>

      <!-- Work queue -->
      <div class="flex flex-col gap-2">
        <div class="flex items-baseline justify-between gap-4">
          <div class="label whitespace-nowrap">Needs attention</div>
          <div class="font-mono text-xs text-ink-tertiary whitespace-nowrap">{{ progressPercent }}% of library filed</div>
        </div>
        <div class="card-ab overflow-hidden">
          <button
            v-for="w in workRows"
            :key="w.key"
            class="flex items-center gap-4 w-full p-4 border-b border-line hover:bg-raised transition-colors text-left"
            @click="w.go()"
          >
            <span class="signal-dot" :style="{ background: w.dot }"></span>
            <span class="font-mono text-sm font-medium text-ink w-12 text-right">{{ w.count }}</span>
            <span class="flex-1 text-[13px] text-ink">
              {{ w.title }}<span class="text-ink-secondary font-light"> — {{ w.sub }}</span>
            </span>
            <span class="font-mono text-xs text-ink-tertiary">→</span>
          </button>
          <div class="flex items-center gap-4 p-4">
            <span class="signal-dot" style="background: var(--status-ready)"></span>
            <span class="font-mono text-sm font-medium text-ink w-12 text-right">{{ stats.confirmed }}</span>
            <span class="flex-1 text-[13px] font-light text-ink-secondary">shots filed and confirmed</span>
          </div>
        </div>
      </div>

      <!-- Scan + people -->
      <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
        <div class="card-ab p-6 flex flex-col gap-4">
          <div class="flex items-center justify-between">
            <div class="label">Library scan</div>
            <span v-if="isScanning" class="font-mono text-[11px] tracking-[0.08em] text-building">
              SCANNING <span class="signal-pulse">●</span>
            </span>
          </div>
          <input
            v-model="libraryPath"
            spellcheck="false"
            :disabled="isScanning"
            class="bg-base border border-line rounded-sm p-2 font-mono text-[13px] text-ink w-full"
          />
          <div v-if="isScanning" class="h-0.5 bg-raised rounded-sm overflow-hidden">
            <div class="h-full bg-signal transition-[width] duration-300" :style="{ width: `${scanProgress}%` }"></div>
          </div>
          <div v-if="scanMessage" class="font-mono text-xs text-ink-secondary">{{ scanMessage }}</div>
          <div v-if="scanError" class="font-mono text-xs text-error">{{ scanError }}</div>
          <div>
            <button
              class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
              :disabled="isScanning"
              @click="startScan"
            >{{ isScanning ? 'Scanning…' : 'Scan library' }}</button>
          </div>
        </div>

        <div class="card-ab p-6 flex flex-col gap-4">
          <div class="flex items-center justify-between">
            <div class="label">People</div>
            <router-link to="/people" class="font-mono text-xs text-signal hover:text-signal-hover">all →</router-link>
          </div>
          <div v-if="people.length" class="flex flex-wrap gap-2">
            <button
              v-for="p in people.slice(0, 12)"
              :key="p.id"
              :title="p.name || 'Unnamed'"
              class="flex flex-col gap-1 items-center w-14"
              @click="router.push({ name: 'person-detail', params: { id: p.id } })"
            >
              <span class="w-12 h-12 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-sm text-ink-tertiary hover:border-signal transition-colors">
                <img v-if="p.thumbnail_url" :src="p.thumbnail_url" class="w-full h-full object-cover" />
                <template v-else>{{ (p.name || '?')[0] }}</template>
              </span>
              <span class="text-[11px] text-ink-secondary truncate max-w-14">{{ p.name || 'unnamed' }}</span>
            </button>
          </div>
          <div v-else class="font-mono text-xs text-ink-tertiary">no people detected yet</div>
        </div>
      </div>

      <div v-if="stats.total_shots === 0" class="flex flex-col items-center gap-2 py-16 text-center">
        <span class="signal-dot" style="width:10px;height:10px;background:var(--status-stopped)"></span>
        <div class="font-heading text-base font-semibold text-ink">Nothing indexed yet</div>
        <div class="text-[13px] font-light text-ink-secondary">Scan a directory above to start filing photos.</div>
      </div>
    </template>
  </div>
</template>

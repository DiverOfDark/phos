<script setup>
import { ref, computed, onMounted, defineExpose } from 'vue'
import { useRouter } from 'vue-router'

const router = useRouter()

const shots = ref([])
const loading = ref(false)
const error = ref(null)

async function fetchPhotos() {
  loading.value = true
  error.value = null
  try {
    const res = await fetch('/api/shots')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    shots.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch shots for timeline', e)
    error.value = e.message
  } finally {
    loading.value = false
  }
}

/**
 * Group shots by date. Uses the timestamp field (DATETIME string) from the API.
 * Falls back to "Unknown date" when no timestamp is present.
 * Returns an array of { date: string, label: string, shots: [] } sorted newest-first.
 */
const groupedByDate = computed(() => {
  const groups = {}

  for (const shot of shots.value) {
    let dateKey = 'Unknown date'
    let sortKey = '0000-00-00'

    if (shot.timestamp) {
      try {
        const d = new Date(shot.timestamp)
        if (!isNaN(d.getTime())) {
          dateKey = d.toLocaleDateString('en-US', {
            weekday: 'long',
            year: 'numeric',
            month: 'long',
            day: 'numeric',
          })
          // ISO date for sorting (newest first)
          sortKey = d.toISOString().slice(0, 10)
        }
      } catch {
        // keep defaults
      }
    }

    if (!groups[sortKey]) {
      groups[sortKey] = { date: sortKey, label: dateKey, shots: [] }
    }
    groups[sortKey].shots.push(shot)
  }

  // Sort groups by date descending (newest first)
  return Object.values(groups).sort((a, b) => b.date.localeCompare(a.date))
})

function openShot(shot) {
  router.push({ name: 'shot-detail', params: { id: shot.id } })
}

onMounted(fetchPhotos)

defineExpose({ fetchPhotos })
</script>

<template>
  <!-- Loading state -->
  <div v-if="loading" class="flex items-center justify-center py-20">
    <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
  </div>

  <!-- Error state -->
  <div v-else-if="error" class="text-center py-20">
    <p class="text-zinc-500 text-sm">Could not load timeline. Is the backend running?</p>
    <p class="text-zinc-600 text-xs mt-2">{{ error }}</p>
  </div>

  <!-- Empty state -->
  <div v-else-if="shots.length === 0" class="text-center py-20">
    <p class="text-zinc-500 text-sm">No photos found. Try scanning a library folder first.</p>
  </div>

  <!-- Timeline content -->
  <div v-else class="space-y-10">
    <section v-for="group in groupedByDate" :key="group.date">
      <!-- Date header -->
      <div class="sticky top-16 z-10 mb-4">
        <div class="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-zinc-900/80 backdrop-blur-md border border-white/5 shadow-lg">
          <div class="w-2 h-2 rounded-full bg-indigo-500"></div>
          <h3 class="text-sm font-semibold text-zinc-200 tracking-wide">{{ group.label }}</h3>
          <span class="text-xs text-zinc-500 ml-1">({{ group.shots.length }})</span>
        </div>
      </div>

      <!-- Shot grid for this date -->
      <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4">
        <div
          v-for="shot in group.shots"
          :key="shot.id"
          class="aspect-square bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800 hover:border-indigo-500 transition-colors cursor-pointer group relative"
          @click="openShot(shot)"
        >
          <img
            :src="shot.thumbnail_url || `/api/files/${shot.id}/thumbnail`"
            :alt="shot.timestamp || 'Shot'"
            class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300"
            loading="lazy"
          />
          <!-- File count badge -->
          <div
            v-if="shot.file_count > 1"
            class="absolute top-2 right-2 px-1.5 py-0.5 rounded bg-black/60 backdrop-blur-sm border border-white/10 text-xs font-medium text-white"
          >
            {{ shot.file_count }}
          </div>
          <!-- Review status dot -->
          <div
            class="absolute top-2 left-2 w-2 h-2 rounded-full"
            :class="shot.review_status === 'confirmed' ? 'bg-emerald-500' : 'bg-yellow-500'"
          />
        </div>
      </div>
    </section>
  </div>
</template>

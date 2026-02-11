<script setup>
import { ref, computed, onMounted, defineExpose } from 'vue'

const photos = ref([])
const loading = ref(false)
const error = ref(null)

async function fetchPhotos() {
  loading.value = true
  error.value = null
  try {
    const res = await fetch('/api/photos')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    photos.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch photos for timeline', e)
    error.value = e.message
  } finally {
    loading.value = false
  }
}

/**
 * Group photos by date. Uses the timestamp field (DATETIME string) from the API.
 * Falls back to "Unknown date" when no timestamp is present.
 * Returns an array of { date: string, label: string, photos: [] } sorted newest-first.
 */
const groupedByDate = computed(() => {
  const groups = {}

  for (const photo of photos.value) {
    let dateKey = 'Unknown date'
    let sortKey = '0000-00-00'

    if (photo.timestamp) {
      try {
        const d = new Date(photo.timestamp)
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
      groups[sortKey] = { date: sortKey, label: dateKey, photos: [] }
    }
    groups[sortKey].photos.push(photo)
  }

  // Sort groups by date descending (newest first)
  return Object.values(groups).sort((a, b) => b.date.localeCompare(a.date))
})

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
  <div v-else-if="photos.length === 0" class="text-center py-20">
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
          <span class="text-xs text-zinc-500 ml-1">({{ group.photos.length }})</span>
        </div>
      </div>

      <!-- Photo grid for this date -->
      <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4">
        <div
          v-for="photo in group.photos"
          :key="photo.id"
          class="aspect-square bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800 hover:border-indigo-500 transition-colors cursor-pointer group"
        >
          <img
            :src="photo.thumbnail_url || `/api/files/${photo.id}/thumbnail`"
            :alt="photo.timestamp || 'Photo'"
            class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300"
            loading="lazy"
          />
        </div>
      </div>
    </section>
  </div>
</template>

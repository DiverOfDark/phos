<script setup>
import { ref, onMounted, watch, defineExpose } from 'vue'
import { useRouter } from 'vue-router'

const router = useRouter()

const props = defineProps({
  personId: { type: String, default: null },
})

const shots = ref([])
const loading = ref(false)
const error = ref(null)

async function fetchPhotos() {
  loading.value = true
  error.value = null
  try {
    let url = '/api/shots'
    if (props.personId) {
      url += `?person_id=${encodeURIComponent(props.personId)}`
    }
    const res = await fetch(url)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    shots.value = await res.json()
  } catch (e) {
    console.error("Failed to fetch shots", e)
    error.value = e.message
  } finally {
    loading.value = false
  }
}

function openShot(shot) {
  router.push({ name: 'shot-detail', params: { id: shot.id } })
}

watch(() => props.personId, fetchPhotos)

onMounted(fetchPhotos)

defineExpose({ fetchPhotos })
</script>

<template>
  <div v-if="loading" class="flex items-center justify-center py-20">
    <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
  </div>
  <div v-else-if="error" class="text-center py-20">
    <p class="text-zinc-500 text-sm">Could not load photos. Is the backend running?</p>
  </div>
  <div v-else-if="shots.length === 0" class="text-center py-20">
    <p class="text-zinc-500 text-sm">No photos found. Try scanning a library folder first.</p>
  </div>
  <div v-else>
    <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4">
      <div
        v-for="shot in shots"
        :key="shot.id"
        class="aspect-square bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800 hover:border-indigo-500 transition-colors cursor-pointer group relative"
        @click="openShot(shot)"
      >
        <img :src="shot.thumbnail_url || `/api/files/${shot.id}/thumbnail`" class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300" loading="lazy" />
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
  </div>
</template>

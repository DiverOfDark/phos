<script setup>
import { ref, onMounted, defineExpose } from 'vue'

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
    console.error("Failed to fetch photos", e)
    error.value = e.message
  } finally {
    loading.value = false
  }
}

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
  <div v-else-if="photos.length === 0" class="text-center py-20">
    <p class="text-zinc-500 text-sm">No photos found. Try scanning a library folder first.</p>
  </div>
  <div v-else class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4">
    <div v-for="photo in photos" :key="photo.id" class="aspect-square bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800 hover:border-indigo-500 transition-colors cursor-pointer group">
       <img :src="photo.thumbnail_url || `/api/files/${photo.id}/thumbnail`" class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300" loading="lazy" />
    </div>
  </div>
</template>

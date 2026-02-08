<script setup>
import { ref, onMounted } from 'vue'

const photos = ref([])

async function fetchPhotos() {
  try {
    const res = await fetch('http://localhost:3000/api/photos')
    photos.value = await res.json()
  } catch (e) {
    console.error("Failed to fetch photos", e)
  }
}

onMounted(fetchPhotos)
</script>

<template>
  <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-4">
    <div v-for="photo in photos" :key="photo.id" class="aspect-square bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800 hover:border-indigo-500 transition-colors cursor-pointer">
       <img :src="`http://localhost:3000/${photo.id}`" class="w-full h-full object-cover" />
    </div>
  </div>
</template>

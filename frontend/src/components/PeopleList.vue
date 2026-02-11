<script setup>
import { ref, onMounted, defineExpose } from 'vue'

const people = ref([])
const loading = ref(false)
const error = ref(null)

async function fetchPeople() {
  loading.value = true
  error.value = null
  try {
    const res = await fetch('/api/people')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    people.value = await res.json()
  } catch (e) {
    console.error("Failed to fetch people", e)
    error.value = e.message
  } finally {
    loading.value = false
  }
}

onMounted(fetchPeople)

defineExpose({ fetchPeople })
</script>

<template>
  <div v-if="loading" class="flex items-center justify-center py-20">
    <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
  </div>
  <div v-else-if="error" class="text-center py-20">
    <p class="text-zinc-500 text-sm">Could not load people. Is the backend running?</p>
  </div>
  <div v-else-if="people.length === 0" class="text-center py-20">
    <p class="text-zinc-500 text-sm">No people detected yet. Scan a library to detect faces.</p>
  </div>
  <div v-else class="grid grid-cols-2 sm:grid-cols-4 md:grid-cols-6 gap-6">
    <div v-for="person in people" :key="person.id" class="flex flex-col items-center space-y-2 group cursor-pointer">
      <div class="w-24 h-24 rounded-full bg-zinc-800 border-2 border-zinc-700 group-hover:border-indigo-500 overflow-hidden flex items-center justify-center transition-colors">
        <span v-if="!person.thumbnail" class="text-2xl font-bold text-zinc-600">{{ (person.name || '?')[0] }}</span>
        <img v-else :src="person.thumbnail" class="w-full h-full object-cover" />
      </div>
      <span class="text-sm font-medium text-zinc-300 group-hover:text-white transition-colors">{{ person.name || 'Unnamed' }}</span>
    </div>
  </div>
</template>

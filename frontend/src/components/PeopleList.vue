<script setup>
import { ref, onMounted } from 'vue'

const people = ref([])

async function fetchPeople() {
  try {
    const res = await fetch('http://localhost:3000/api/people')
    people.value = await res.json()
  } catch (e) {
    console.error("Failed to fetch people", e)
  }
}

onMounted(fetchPeople)
</script>

<template>
  <div class="grid grid-cols-2 sm:grid-cols-4 md:grid-cols-6 gap-6">
    <div v-for="person in people" :key="person.id" class="flex flex-col items-center space-y-2">
      <div class="w-24 h-24 rounded-full bg-zinc-800 border-2 border-zinc-700 overflow-hidden flex items-center justify-center">
        <span v-if="!person.thumbnail" class="text-2xl font-bold text-zinc-600">?</span>
        <img v-else :src="person.thumbnail" class="w-full h-full object-cover" />
      </div>
      <span class="text-sm font-medium">{{ person.name || 'Unnamed' }}</span>
    </div>
  </div>
</template>

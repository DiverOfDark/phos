<script setup>
import { ref, computed, onMounted, defineExpose } from 'vue'
import { useRouter } from 'vue-router'
import { Button } from '@/components/ui/button'
import { UserPlus } from 'lucide-vue-next'
import PersonNamer from '@/components/PersonNamer.vue'

const router = useRouter()

const people = ref([])
const loading = ref(false)
const error = ref(null)
const showNamer = ref(false)

const hasUnnamed = computed(() => people.value.some((p) => !p.name))

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

function onNamerChanged() {
  fetchPeople()
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
  <div v-else>
    <!-- Header with Name People button -->
    <div v-if="hasUnnamed" class="flex justify-end mb-4">
      <Button
        @click="showNamer = true"
        class="bg-indigo-600 hover:bg-indigo-500 text-white gap-2"
      >
        <UserPlus class="w-4 h-4" />
        Name People
      </Button>
    </div>

    <div class="grid grid-cols-2 sm:grid-cols-4 md:grid-cols-6 gap-6">
      <div v-for="person in people" :key="person.id" class="flex flex-col items-center space-y-2 group cursor-pointer" @click="router.push({ name: 'person-detail', params: { id: person.id } })">
        <div class="w-24 h-24 rounded-full bg-zinc-800 border-2 border-zinc-700 group-hover:border-indigo-500 overflow-hidden flex items-center justify-center transition-colors">
          <img
            v-if="person.thumbnail_url"
            :src="person.thumbnail_url"
            class="w-full h-full object-cover"
          />
          <span v-else class="text-2xl font-bold text-zinc-600">{{ (person.name || '?')[0] }}</span>
        </div>
        <span class="text-sm font-medium text-zinc-300 group-hover:text-white transition-colors">{{ person.name || 'Unnamed' }}</span>
        <div class="flex items-center gap-2">
          <span class="text-xs text-zinc-500">{{ person.shot_count || 0 }} {{ (person.shot_count || 0) === 1 ? 'shot' : 'shots' }}</span>
          <span v-if="person.face_count" class="text-xs text-zinc-600">{{ person.face_count }} {{ person.face_count === 1 ? 'face' : 'faces' }}</span>
        </div>
        <span v-if="(person.pending_count || 0) > 0" class="text-[10px] text-yellow-500">{{ person.pending_count }} pending</span>
      </div>
    </div>

    <!-- PersonNamer dialog -->
    <PersonNamer
      v-model:open="showNamer"
      @changed="onNamerChanged"
    />
  </div>
</template>

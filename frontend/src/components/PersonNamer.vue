<script setup>
import { ref, computed, watch } from 'vue'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Check, SkipForward, Search } from 'lucide-vue-next'

const props = defineProps({
  open: { type: Boolean, default: false },
})

const emit = defineEmits(['update:open', 'changed'])

const dialogOpen = computed({
  get: () => props.open,
  set: (val) => emit('update:open', val),
})

const allPeople = ref([])
const unnamedPeople = ref([])
const namedPeople = ref([])
const currentIndex = ref(0)
const faces = ref([])
const newName = ref('')
const mergeFilter = ref('')
const loading = ref(false)
const done = ref(false)

const currentPerson = computed(() => unnamedPeople.value[currentIndex.value] || null)

const progress = computed(() => {
  const total = unnamedPeople.value.length
  if (total === 0) return { current: 0, total: 0 }
  return { current: Math.min(currentIndex.value + 1, total), total }
})

const filteredNamedPeople = computed(() => {
  const q = mergeFilter.value.toLowerCase()
  if (!q) return namedPeople.value
  return namedPeople.value.filter((p) => p.name && p.name.toLowerCase().includes(q))
})

watch(dialogOpen, async (isOpen) => {
  if (isOpen) {
    currentIndex.value = 0
    done.value = false
    newName.value = ''
    mergeFilter.value = ''
    await fetchPeople()
    if (unnamedPeople.value.length > 0) {
      await fetchFaces(unnamedPeople.value[0].id)
    } else {
      done.value = true
    }
  }
})

async function fetchPeople() {
  loading.value = true
  try {
    const res = await fetch('/api/people')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    allPeople.value = await res.json()
    unnamedPeople.value = allPeople.value.filter((p) => !p.name)
    namedPeople.value = allPeople.value.filter((p) => p.name)
  } catch (e) {
    console.error('Failed to fetch people', e)
  } finally {
    loading.value = false
  }
}

async function fetchFaces(personId) {
  try {
    const res = await fetch(`/api/people/${personId}/faces`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    faces.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch faces', e)
    faces.value = []
  }
}

async function namePerson() {
  const name = newName.value.trim()
  if (!name || !currentPerson.value) return

  try {
    const res = await fetch(`/api/people/${currentPerson.value.id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    // Add to named list
    namedPeople.value.push({ ...currentPerson.value, name })
    newName.value = ''
    emit('changed')
    await advance()
  } catch (e) {
    console.error('Failed to name person', e)
  }
}

async function mergePerson(targetId) {
  if (!currentPerson.value) return

  try {
    const res = await fetch('/api/people/merge', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        source_id: currentPerson.value.id,
        target_id: targetId,
      }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    // Remove merged person from unnamed list
    unnamedPeople.value.splice(currentIndex.value, 1)
    emit('changed')

    // Don't advance index since we removed the current item
    if (currentIndex.value >= unnamedPeople.value.length) {
      done.value = true
    } else {
      await fetchFaces(unnamedPeople.value[currentIndex.value].id)
    }
  } catch (e) {
    console.error('Failed to merge person', e)
  }
}

async function skip() {
  await advance()
}

async function advance() {
  const nextIdx = currentIndex.value + 1
  if (nextIdx >= unnamedPeople.value.length) {
    done.value = true
  } else {
    currentIndex.value = nextIdx
    newName.value = ''
    await fetchFaces(unnamedPeople.value[nextIdx].id)
  }
}
</script>

<template>
  <Dialog v-model:open="dialogOpen">
    <DialogContent class="sm:max-w-[520px] max-h-[85vh] overflow-hidden flex flex-col">
      <DialogHeader>
        <DialogTitle>Name People</DialogTitle>
        <DialogDescription>
          Identify detected faces by naming them or merging with existing people.
        </DialogDescription>
      </DialogHeader>

      <!-- Done state -->
      <div v-if="done" class="flex flex-col items-center justify-center py-12 space-y-4">
        <div class="w-16 h-16 rounded-full bg-emerald-500/10 flex items-center justify-center">
          <Check class="w-8 h-8 text-emerald-500" />
        </div>
        <p class="text-lg font-semibold text-white">All done!</p>
        <p class="text-sm text-zinc-400">All people have been reviewed.</p>
        <Button @click="dialogOpen = false" class="bg-indigo-600 hover:bg-indigo-500 text-white">
          Close
        </Button>
      </div>

      <!-- Loading -->
      <div v-else-if="loading" class="flex items-center justify-center py-12">
        <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
      </div>

      <!-- Wizard content -->
      <div v-else-if="currentPerson" class="flex flex-col gap-4 min-h-0">
        <!-- Progress -->
        <div class="flex items-center justify-between text-sm">
          <span class="text-zinc-400">
            {{ progress.current }} of {{ progress.total }} unnamed people
          </span>
          <Button variant="ghost" size="sm" class="text-zinc-400 hover:text-white gap-1" @click="skip">
            <SkipForward class="w-3.5 h-3.5" />
            Skip
          </Button>
        </div>
        <div class="w-full bg-zinc-800 h-1 rounded-full overflow-hidden">
          <div
            class="bg-indigo-500 h-full transition-all duration-300"
            :style="{ width: `${(progress.current / progress.total) * 100}%` }"
          ></div>
        </div>

        <!-- Face thumbnails -->
        <div class="flex flex-wrap gap-2 justify-center">
          <div
            v-for="face in faces"
            :key="face.id"
            class="w-16 h-16 rounded-lg overflow-hidden border border-white/10 bg-zinc-800"
          >
            <img
              :src="face.thumbnail_url"
              class="w-full h-full object-cover"
              loading="lazy"
            />
          </div>
          <div
            v-if="faces.length === 0"
            class="text-sm text-zinc-500 py-4"
          >
            No face thumbnails available
          </div>
        </div>

        <!-- Name input -->
        <div class="flex gap-2">
          <Input
            v-model="newName"
            placeholder="Type a name..."
            class="flex-1"
            @keydown.enter="namePerson"
          />
          <Button
            @click="namePerson"
            :disabled="!newName.trim()"
            class="bg-indigo-600 hover:bg-indigo-500 text-white shrink-0"
          >
            Name
          </Button>
        </div>

        <!-- Merge section -->
        <div v-if="namedPeople.length > 0" class="flex flex-col gap-2 min-h-0">
          <p class="text-xs font-medium text-zinc-400 uppercase tracking-wider">Or merge with existing person</p>
          <div class="relative">
            <Search class="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500" />
            <Input
              v-model="mergeFilter"
              placeholder="Filter people..."
              class="pl-9 h-8 text-sm"
            />
          </div>
          <ScrollArea class="max-h-40 rounded-lg border border-white/5">
            <div class="p-1 space-y-0.5">
              <button
                v-for="person in filteredNamedPeople"
                :key="person.id"
                @click="mergePerson(person.id)"
                class="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-left hover:bg-white/5 transition-colors group"
              >
                <div class="w-8 h-8 rounded-full bg-zinc-800 border border-white/10 overflow-hidden flex items-center justify-center shrink-0">
                  <img
                    v-if="person.thumbnail_url"
                    :src="person.thumbnail_url"
                    class="w-full h-full object-cover"
                  />
                  <span v-else class="text-xs font-bold text-zinc-500">{{ (person.name || '?')[0] }}</span>
                </div>
                <span class="text-sm text-zinc-300 group-hover:text-white truncate">{{ person.name }}</span>
                <span class="text-xs text-zinc-600 ml-auto shrink-0">{{ person.face_count }} faces</span>
              </button>
              <p v-if="filteredNamedPeople.length === 0" class="text-xs text-zinc-500 text-center py-3">
                No matching people
              </p>
            </div>
          </ScrollArea>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>

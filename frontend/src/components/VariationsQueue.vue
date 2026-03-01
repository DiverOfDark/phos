<script setup>
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import { Check, X, Merge, Layers } from 'lucide-vue-next'

const router = useRouter()
const groups = ref([])
const currentIndex = ref(0)
const loading = ref(true)
const totalGroups = ref(0)
const pageOffset = ref(0)
const pageLimit = 50

// For the current group, track which candidates are selected for merge
const selectedCandidates = ref(new Set())

const currentGroup = computed(() => groups.value[currentIndex.value] || null)

onMounted(async () => {
  window.addEventListener('keydown', handleKeydown)
  await fetchGroups()
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown)
})

async function fetchGroups(append = false) {
  loading.value = true
  try {
    const offset = append ? pageOffset.value : 0
    const res = await fetch(`/api/shots/similar-groups?offset=${offset}&limit=${pageLimit}`)
    if (res.ok) {
      const data = await res.json()
      if (append) {
        groups.value.push(...data.groups)
      } else {
        groups.value = data.groups
        currentIndex.value = 0
      }
      totalGroups.value = data.total
      pageOffset.value = offset + data.groups.length
      initSelection()
    }
  } catch (e) {
    console.error('Failed to fetch groups', e)
  }
  loading.value = false
}

function initSelection() {
  if (currentGroup.value) {
    selectedCandidates.value = new Set(currentGroup.value.candidates.map(c => c.id))
  }
}

function toggleCandidate(id) {
  if (selectedCandidates.value.has(id)) {
    selectedCandidates.value.delete(id)
  } else {
    selectedCandidates.value.add(id)
  }
}

function setAsPrimary(candidate) {
  if (!currentGroup.value) return
  // Move current primary to candidates
  const oldPrimary = currentGroup.value.primary
  currentGroup.value.primary = candidate
  currentGroup.value.candidates = currentGroup.value.candidates.filter(c => c.id !== candidate.id)
  currentGroup.value.candidates.push(oldPrimary)
  initSelection()
}

async function handleMerge() {
  if (!currentGroup.value) return
  const primaryId = currentGroup.value.primary.id
  
  // Merge selected
  for (const c of currentGroup.value.candidates) {
    if (selectedCandidates.value.has(c.id)) {
      await fetch('/api/shots/merge', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source_id: c.id, target_id: primaryId })
      })
    } else {
      // Ignore unselected
      await fetch('/api/shots/merge/ignore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ shot_id_1: primaryId, shot_id_2: c.id })
      })
    }
  }
  nextGroup()
}

async function handleIgnoreAll() {
  if (!currentGroup.value) return
  const primaryId = currentGroup.value.primary.id
  for (const c of currentGroup.value.candidates) {
    await fetch('/api/shots/merge/ignore', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ shot_id_1: primaryId, shot_id_2: c.id })
    })
  }
  nextGroup()
}

function nextGroup() {
  currentIndex.value++
  initSelection()
  if (currentIndex.value >= groups.value.length) {
    if (pageOffset.value < totalGroups.value) {
      fetchGroups(true) // fetch next batch, appending
    }
  }
}

function handleKeydown(e) {
  if (!currentGroup.value) return
  if (e.key === 'Enter') {
    handleMerge()
  } else if (e.key === 'Escape') {
    handleIgnoreAll()
  }
}

function viewShot(id) {
  window.open(`/shot/${id}`, '_blank')
}
</script>

<template>
  <div class="h-full flex flex-col bg-black text-white">
    <header class="h-14 border-b border-white/10 flex items-center justify-between px-6 shrink-0">
      <div class="flex items-center gap-4">
        <h1 class="text-lg font-medium tracking-tight">Variations Queue</h1>
        <span v-if="!loading && groups.length > 0" class="text-sm text-zinc-500">
          Group {{ currentIndex + 1 }} of {{ totalGroups }}
        </span>
      </div>
      <div class="flex items-center gap-3 text-sm text-zinc-400">
        <span class="flex items-center gap-1"><kbd class="bg-zinc-800 px-1.5 py-0.5 rounded text-xs border border-zinc-700">Enter</kbd> Merge Selected</span>
        <span class="flex items-center gap-1"><kbd class="bg-zinc-800 px-1.5 py-0.5 rounded text-xs border border-zinc-700">Esc</kbd> Ignore All</span>
      </div>
    </header>

    <div class="flex-1 flex flex-col min-h-0">
      <div v-if="loading" class="flex flex-1 items-center justify-center text-zinc-500">
        Finding similar shots...
      </div>
      <div v-else-if="groups.length === 0" class="flex flex-col flex-1 items-center justify-center text-zinc-500 gap-4">
        <Layers class="w-12 h-12 text-zinc-700" />
        <p>No similar groups found.</p>
        <button @click="router.push('/')" class="text-indigo-400 hover:text-indigo-300">Back to Library</button>
      </div>
      <template v-else-if="currentGroup">
        <!-- Primary + actions bar -->
        <div class="shrink-0 flex items-center gap-6 px-6 py-4 border-b border-white/10">
          <div class="relative rounded-lg overflow-hidden border border-indigo-500/50 shadow-[0_0_15px_rgba(99,102,241,0.2)] bg-zinc-900 cursor-pointer shrink-0" @click="viewShot(currentGroup.primary.id)">
            <img :src="currentGroup.primary.thumbnail_url" class="h-32 object-contain" />
            <div class="absolute top-2 left-2 bg-indigo-500 text-white text-[10px] font-bold px-1.5 py-0.5 rounded shadow flex items-center gap-1">
              <Layers class="w-3 h-3" />
              Primary
            </div>
          </div>
          <div class="flex flex-col gap-2">
            <span class="text-sm text-zinc-400">{{ currentGroup.primary.file_count }} files</span>
            <button @click="handleMerge" class="px-5 py-2 bg-indigo-600 hover:bg-indigo-500 text-white text-sm font-medium rounded-lg shadow transition-colors flex items-center gap-2">
              <Merge class="w-4 h-4" />
              Merge {{ selectedCandidates.size }} into Primary
            </button>
            <button @click="handleIgnoreAll" class="text-xs text-zinc-500 hover:text-zinc-300 transition-colors text-left">
              Keep All Separate
            </button>
          </div>
        </div>

        <!-- Candidates grid — takes all remaining space and scrolls -->
        <div class="flex-1 min-h-0 overflow-y-auto p-6">
          <div class="grid gap-4" style="grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));">
            <div
              v-for="candidate in currentGroup.candidates"
              :key="candidate.id"
              class="relative rounded-lg border transition-all duration-200 cursor-pointer bg-zinc-900 flex flex-col"
              :class="selectedCandidates.has(candidate.id) ? 'border-indigo-500 shadow-[0_0_10px_rgba(99,102,241,0.3)]' : 'border-white/10 opacity-60 hover:opacity-100'"
              @click="toggleCandidate(candidate.id)"
            >
              <div class="overflow-hidden rounded-t-lg">
                <img :src="candidate.thumbnail_url" class="w-full aspect-[4/3] object-cover" />
              </div>

              <div class="absolute top-2 left-2 flex items-center justify-center w-6 h-6 rounded-full border-2"
                   :class="selectedCandidates.has(candidate.id) ? 'bg-indigo-500 border-indigo-500 text-white' : 'bg-black/50 border-white/30 text-transparent'">
                <Check class="w-4 h-4" v-if="selectedCandidates.has(candidate.id)" />
              </div>

              <div class="p-2.5 border-t border-white/10 text-xs flex justify-between items-center gap-2">
                <span class="text-zinc-400 whitespace-nowrap">{{ candidate.file_count }} files</span>
                <button
                  @click.stop="setAsPrimary(candidate)"
                  class="text-indigo-400 hover:text-indigo-300 font-medium whitespace-nowrap"
                >
                  Make Primary
                </button>
              </div>
            </div>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

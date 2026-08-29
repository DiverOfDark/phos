<script setup>
/**
 * Duplicates lane — one group at a time: a primary shot and the near-duplicates
 * the similarity search paired with it. Selected candidates merge into the
 * primary; unselected pairs are remembered as deliberately distinct.
 */
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'

const emit = defineEmits(['changed'])

const router = useRouter()
const groups = ref([])
const currentIndex = ref(0)
const loading = ref(true)
const totalGroups = ref(0)
const pageOffset = ref(0)
const pageLimit = 50
const handled = ref(0)

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
  // Set mutations are not reactive on their own.
  selectedCandidates.value = new Set(selectedCandidates.value)
}

function setAsPrimary(candidate) {
  if (!currentGroup.value) return
  const oldPrimary = currentGroup.value.primary
  currentGroup.value.primary = candidate
  currentGroup.value.candidates = currentGroup.value.candidates.filter(c => c.id !== candidate.id)
  currentGroup.value.candidates.push(oldPrimary)
  initSelection()
}

async function handleMerge() {
  if (!currentGroup.value) return
  const primaryId = currentGroup.value.primary.id

  for (const c of currentGroup.value.candidates) {
    if (selectedCandidates.value.has(c.id)) {
      await fetch('/api/shots/merge', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source_id: c.id, target_id: primaryId })
      })
    } else {
      await fetch('/api/shots/merge/ignore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ shot_id_1: primaryId, shot_id_2: c.id })
      })
    }
  }
  emit('changed')
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
  emit('changed')
  nextGroup()
}

function nextGroup() {
  handled.value++
  currentIndex.value++
  initSelection()
  if (currentIndex.value >= groups.value.length) {
    if (pageOffset.value < totalGroups.value) {
      fetchGroups(true)
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
  router.push(`/shot/${id}`)
}

defineExpose({ loadData: () => fetchGroups() })
</script>

<template>
  <div class="flex flex-col flex-1 min-h-0 overflow-y-auto">
    <div v-if="loading" class="flex-1 flex items-center justify-center py-16">
      <span class="font-mono text-xs text-ink-tertiary">
        finding similar shots <span class="text-building signal-pulse">●</span>
      </span>
    </div>

    <div v-else-if="!currentGroup" class="flex-1 flex flex-col items-center justify-center gap-2 p-16 text-center">
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-ready)"></span>
      <div class="font-heading text-base font-semibold text-ink">No duplicate groups left</div>
      <div class="text-[13px] font-light text-ink-secondary">Similarity search runs after every scan.</div>
    </div>

    <div v-else class="flex-1 p-4 md:p-8 flex flex-col gap-6 max-w-[1040px] w-full mx-auto">
      <div class="flex items-baseline justify-between gap-4">
        <div class="text-[13px] font-light text-ink-secondary">
          Selected shots merge into the primary; unselected pairs are remembered as distinct.
        </div>
        <div class="font-mono text-xs text-ink-tertiary whitespace-nowrap">
          group {{ currentIndex + 1 }} / {{ totalGroups }}
        </div>
      </div>

      <div class="flex flex-col md:flex-row gap-6 items-start">
        <div class="flex-none flex flex-col gap-2">
          <button
            class="w-[280px] max-w-full aspect-[4/3] bg-surface border rounded overflow-hidden flex items-center justify-center relative p-0"
            style="border-color: var(--accent-muted)"
            @click="viewShot(currentGroup.primary.id)"
          >
            <img :src="currentGroup.primary.thumbnail_url" class="w-full h-full object-contain" />
            <span class="tag absolute top-2 left-2 bg-base" style="color: var(--accent); border-color: var(--accent-muted)">Primary</span>
          </button>
          <div class="font-mono text-[11px] text-ink-tertiary">{{ currentGroup.primary.file_count }} files</div>
        </div>

        <div class="flex-1 grid gap-4 w-full" style="grid-template-columns: repeat(auto-fill, minmax(160px, 1fr))">
          <div v-for="candidate in currentGroup.candidates" :key="candidate.id" class="flex flex-col gap-1">
            <button
              class="aspect-[4/3] bg-surface border rounded overflow-hidden flex items-center justify-center relative p-0 transition-opacity"
              :class="selectedCandidates.has(candidate.id) ? 'border-signal opacity-100' : 'border-line opacity-50'"
              @click="toggleCandidate(candidate.id)"
            >
              <img :src="candidate.thumbnail_url" class="w-full h-full object-cover" />
              <span
                class="absolute top-2 left-2 w-3.5 h-3.5 rounded-sm border flex items-center justify-center text-[10px]"
                :class="selectedCandidates.has(candidate.id)
                  ? 'bg-signal border-signal text-signal-fg'
                  : 'bg-base border-line-strong text-transparent'"
              >✓</span>
            </button>
            <div class="flex justify-between items-center gap-2">
              <span class="font-mono text-[11px] text-ink-tertiary">{{ candidate.file_count }} files</span>
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                @click.stop="setAsPrimary(candidate)"
              >make primary</button>
            </div>
          </div>
        </div>
      </div>

      <div class="flex flex-wrap gap-2 border-t border-line pt-4">
        <button
          class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors"
          @click="handleMerge"
        >
          Merge {{ selectedCandidates.size }} into primary
          <kbd class="ml-1 font-mono text-[10px] border rounded-sm px-1" style="border-color: oklch(15% 0.01 80 / .3)">⏎</kbd>
        </button>
        <button
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
          @click="handleIgnoreAll"
        >
          Keep all separate
          <kbd class="ml-1 kbd-ab">esc</kbd>
        </button>
        <span class="flex-1"></span>
        <span v-if="handled" class="font-mono text-[11px] text-ink-tertiary self-center">{{ handled }} groups handled this session</span>
      </div>
    </div>
  </div>
</template>

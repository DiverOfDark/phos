<script setup>
/**
 * Faces lane — name an unnamed cluster, or merge it into a person who already
 * has a name. Renders inline inside the Review Desk; the dialog form is kept for
 * callers that still open it as a modal.
 */
import { ref, computed, watch, onMounted } from 'vue'

const props = defineProps({
  open: { type: Boolean, default: false },
  inline: { type: Boolean, default: false },
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
const handled = ref(0)

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

async function start() {
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

watch(dialogOpen, async (isOpen) => {
  if (isOpen) await start()
})

onMounted(() => {
  if (props.inline) start()
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

    namedPeople.value.push({ ...currentPerson.value, name })
    newName.value = ''
    handled.value++
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

    unnamedPeople.value.splice(currentIndex.value, 1)
    handled.value++
    emit('changed')

    // The splice already moved the next cluster into this index.
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

defineExpose({ loadData: start })
</script>

<template>
  <!-- Inline: the Review Desk's Faces lane -->
  <div v-if="inline" class="flex flex-col flex-1 min-h-0 overflow-y-auto">
    <div v-if="loading" class="flex-1 flex items-center justify-center py-16">
      <span class="font-mono text-xs text-ink-tertiary">loading clusters…</span>
    </div>

    <div v-else-if="done" class="flex-1 flex flex-col items-center justify-center gap-2 p-16 text-center">
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-ready)"></span>
      <div class="font-heading text-base font-semibold text-ink">Every cluster has a name</div>
      <div class="text-[13px] font-light text-ink-secondary">
        New clusters appear here when unknown faces are detected.
      </div>
    </div>

    <div v-else-if="currentPerson" class="flex-1 p-4 md:p-8 flex flex-col gap-6 max-w-[800px] w-full mx-auto">
      <div class="flex items-baseline justify-between gap-4">
        <div class="text-[13px] font-light text-ink-secondary">
          Name this cluster, or merge it into an existing person if it is the same face.
        </div>
        <div class="font-mono text-xs text-ink-tertiary whitespace-nowrap">
          cluster/{{ currentPerson.id }} · {{ faces.length }} faces
        </div>
      </div>

      <div class="flex flex-wrap gap-2">
        <div
          v-for="face in faces"
          :key="face.id"
          class="w-[72px] h-[72px] rounded bg-raised border border-line overflow-hidden flex items-center justify-center"
        >
          <img :src="face.thumbnail_url" class="w-full h-full object-cover" loading="lazy" />
        </div>
        <div v-if="faces.length === 0" class="font-mono text-xs text-ink-tertiary py-4">
          no face thumbnails available
        </div>
      </div>

      <div class="flex gap-2">
        <input
          v-model="newName"
          placeholder="Type a name…"
          spellcheck="false"
          class="flex-1 bg-surface border border-line rounded-sm px-3 py-2.5 text-sm text-ink"
          @keydown.enter="namePerson"
        />
        <button
          class="bg-signal text-signal-fg rounded px-6 py-2.5 text-sm font-medium hover:bg-signal-hover transition-colors disabled:opacity-40"
          :disabled="!newName.trim()"
          @click="namePerson"
        >Name</button>
        <button
          class="border border-line-strong rounded px-4 py-2.5 text-[13px] text-ink-secondary hover:text-signal transition-colors"
          @click="skip"
        >Skip</button>
      </div>

      <div v-if="namedPeople.length > 0" class="flex flex-col gap-2">
        <div class="label">Same person as</div>
        <input
          v-model="mergeFilter"
          placeholder="Search people…"
          spellcheck="false"
          class="bg-base border border-line rounded-sm px-3 py-2 text-[13px] text-ink"
        />
        <div class="card-ab overflow-hidden">
          <button
            v-for="person in filteredNamedPeople"
            :key="person.id"
            class="flex items-center gap-2 w-full px-4 py-2 border-b border-line hover:bg-raised transition-colors text-left"
            @click="mergePerson(person.id)"
          >
            <span class="w-6 h-6 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[11px] text-ink-tertiary shrink-0">
              <img v-if="person.thumbnail_url" :src="person.thumbnail_url" class="w-full h-full object-cover" />
              <template v-else>{{ (person.name || '?')[0] }}</template>
            </span>
            <span class="flex-1 text-[13px] text-ink truncate">{{ person.name }}</span>
            <span class="font-mono text-[11px] text-ink-tertiary">{{ person.face_count }} faces</span>
          </button>
          <p v-if="filteredNamedPeople.length === 0" class="font-mono text-[11px] text-ink-tertiary text-center py-3">
            no matching people
          </p>
        </div>
      </div>

      <div class="font-mono text-[11px] text-ink-tertiary">
        {{ progress.current }} / {{ progress.total }} unnamed clusters<span v-if="handled"> · {{ handled }} handled this session</span>
      </div>
    </div>
  </div>

  <!-- Modal form, for callers that still open this as a dialog -->
  <div
    v-else-if="dialogOpen"
    class="fixed inset-0 z-50 flex items-center justify-center p-4"
    style="background: var(--scrim)"
    @click="dialogOpen = false"
  >
    <div
      class="w-[520px] max-w-full max-h-[85vh] bg-overlay border border-line-strong rounded shadow-lg flex flex-col overflow-hidden"
      @click.stop
    >
      <div class="flex items-center justify-between px-6 py-4 border-b border-line">
        <div class="font-heading text-base font-semibold text-ink">Name face clusters</div>
        <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="dialogOpen = false">✕</button>
      </div>

      <div v-if="done" class="flex flex-col items-center justify-center gap-2 py-12 px-6 text-center">
        <span class="signal-dot" style="width:10px;height:10px;background:var(--status-ready)"></span>
        <div class="font-heading text-base font-semibold text-ink">Every cluster has a name</div>
        <button
          class="mt-2 bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors"
          @click="dialogOpen = false"
        >Close</button>
      </div>

      <div v-else-if="loading" class="py-12 text-center font-mono text-xs text-ink-tertiary">loading clusters…</div>

      <div v-else-if="currentPerson" class="p-6 flex flex-col gap-4 overflow-y-auto min-h-0">
        <div class="font-mono text-[11px] text-ink-tertiary">
          {{ progress.current }} / {{ progress.total }} unnamed clusters
        </div>
        <div class="flex flex-wrap gap-2">
          <div
            v-for="face in faces"
            :key="face.id"
            class="w-16 h-16 rounded bg-raised border border-line overflow-hidden"
          >
            <img :src="face.thumbnail_url" class="w-full h-full object-cover" loading="lazy" />
          </div>
        </div>
        <div class="flex gap-2">
          <input
            v-model="newName"
            placeholder="Type a name…"
            spellcheck="false"
            class="flex-1 bg-base border border-line rounded-sm px-3 py-2 text-sm text-ink"
            @keydown.enter="namePerson"
          />
          <button
            class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-40"
            :disabled="!newName.trim()"
            @click="namePerson"
          >Name</button>
          <button
            class="border border-line-strong rounded px-3 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
            @click="skip"
          >Skip</button>
        </div>
        <div v-if="namedPeople.length > 0" class="flex flex-col gap-2 min-h-0">
          <div class="label">Same person as</div>
          <input
            v-model="mergeFilter"
            placeholder="Search people…"
            spellcheck="false"
            class="bg-base border border-line rounded-sm px-3 py-2 text-[13px] text-ink"
          />
          <div class="card-ab overflow-y-auto max-h-40">
            <button
              v-for="person in filteredNamedPeople"
              :key="person.id"
              class="flex items-center gap-2 w-full px-3 py-2 border-b border-line hover:bg-raised transition-colors text-left"
              @click="mergePerson(person.id)"
            >
              <span class="w-6 h-6 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[11px] text-ink-tertiary shrink-0">
                <img v-if="person.thumbnail_url" :src="person.thumbnail_url" class="w-full h-full object-cover" />
                <template v-else>{{ (person.name || '?')[0] }}</template>
              </span>
              <span class="flex-1 text-[13px] text-ink truncate">{{ person.name }}</span>
              <span class="font-mono text-[11px] text-ink-tertiary">{{ person.face_count }} faces</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

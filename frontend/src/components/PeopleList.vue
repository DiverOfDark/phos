<script setup>
/**
 * People — a wall of faces.
 *
 * The design system says the console carries no imagery, but that rule was
 * written for an infrastructure console. Here the imagery *is* the data: a face
 * is the key a human searches by and the name is only its label, so the crop
 * gets the space and the counts go underneath in mono.
 */
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'

const router = useRouter()

const people = ref([])
const unsortedCount = ref(0)
const loading = ref(false)
const error = ref(null)

const query = ref('')
const sort = ref(localStorage.getItem('phos_people_sort') || 'shots')

const SORTS = [
  { id: 'shots', label: 'shots' },
  { id: 'name', label: 'name' },
  { id: 'pending', label: 'pending' },
]

function setSort(id) {
  sort.value = id
  localStorage.setItem('phos_people_sort', id)
}

const namedCount = computed(() => people.value.filter((p) => p.name).length)
const unnamedCount = computed(() => people.value.filter((p) => !p.name).length)

const visiblePeople = computed(() => {
  const q = query.value.trim().toLowerCase()
  const list = q
    ? people.value.filter((p) => (p.name || 'unnamed').toLowerCase().includes(q))
    : [...people.value]
  return list.sort((a, b) => {
    if (sort.value === 'name') {
      // Unnamed clusters have no name to sort by, so they sit at the end rather
      // than clumping under an empty string at the top.
      if (!a.name) return b.name ? 1 : 0
      if (!b.name) return -1
      return a.name.localeCompare(b.name)
    }
    if (sort.value === 'pending') return (b.pending_count || 0) - (a.pending_count || 0)
    return (b.shot_count || 0) - (a.shot_count || 0)
  })
})

async function fetchPeople() {
  loading.value = true
  error.value = null
  try {
    const [peopleRes, statsRes] = await Promise.all([
      fetch('/api/people'),
      fetch('/api/organize/stats'),
    ])
    if (!peopleRes.ok) throw new Error(`HTTP ${peopleRes.status}`)
    people.value = await peopleRes.json()
    if (statsRes.ok) {
      const stats = await statsRes.json()
      unsortedCount.value = stats.unsorted || 0
    }
  } catch (e) {
    console.error('Failed to fetch people', e)
    error.value = e.message
  } finally {
    loading.value = false
  }
}

// --- Hover: cycle through a person's other crops ---
//
// One unlucky cover — a blink, a profile, half a face — makes somebody
// unrecognisable. Hovering rotates through the rest of their faces, fetched once
// per person and only when actually pointed at.
const faceCache = ref({})
const hovered = ref(null)
const cycle = ref(0)
let cycleTimer = null

async function onEnter(person) {
  hovered.value = person.id
  cycle.value = 0
  clearInterval(cycleTimer)
  cycleTimer = setInterval(() => { cycle.value += 1 }, 800)

  if (faceCache.value[person.id]) return
  try {
    const res = await fetch(`/api/people/${person.id}/faces`)
    const faces = res.ok ? await res.json() : []
    faceCache.value = { ...faceCache.value, [person.id]: faces.map((f) => f.thumbnail_url) }
  } catch {
    faceCache.value = { ...faceCache.value, [person.id]: [] }
  }
}

function onLeave() {
  hovered.value = null
  clearInterval(cycleTimer)
  cycleTimer = null
}

onUnmounted(() => clearInterval(cycleTimer))

function cropUrl(person) {
  const crops = faceCache.value[person.id]
  if (hovered.value === person.id && crops && crops.length > 1) {
    return crops[cycle.value % crops.length]
  }
  return person.thumbnail_url || person.cover_shot_thumbnail_url || null
}

onMounted(fetchPeople)

defineExpose({ fetchPeople, loadData: fetchPeople })
</script>

<template>
  <div class="p-4 md:p-8 max-w-[1040px] w-full mx-auto flex flex-col gap-6">
    <div class="flex flex-wrap items-baseline justify-between gap-4">
      <h2 class="text-[22px] font-semibold">People</h2>
      <div class="font-mono text-xs text-ink-tertiary">
        {{ namedCount }} named · {{ unnamedCount }} unnamed clusters
      </div>
    </div>

    <div v-if="loading" class="font-mono text-xs text-ink-tertiary py-16 text-center">loading people…</div>
    <div v-else-if="error" class="font-mono text-xs text-error py-16 text-center">{{ error }}</div>

    <template v-else>
      <button
        v-if="unnamedCount > 0"
        class="flex items-center gap-4 p-4 card-ab hover:bg-raised transition-colors text-left"
        @click="router.push({ path: '/review', query: { lane: 'faces' } })"
      >
        <span class="signal-dot signal-pulse" style="background: var(--status-degraded)"></span>
        <span class="flex-1 text-[13px] text-ink">
          {{ unnamedCount }} unnamed face clusters<span class="text-ink-secondary font-light"> — name or merge them in the Review Desk</span>
        </span>
        <span class="font-mono text-xs text-ink-tertiary">→</span>
      </button>

      <button
        v-if="unsortedCount > 0"
        class="flex items-center gap-4 p-4 card-ab hover:bg-raised transition-colors text-left"
        @click="router.push({ path: '/review', query: { status: 'unsorted' } })"
      >
        <span class="signal-dot" style="background: var(--status-pending)"></span>
        <span class="flex-1 text-[13px] text-ink">
          {{ unsortedCount }} unsorted shots<span class="text-ink-secondary font-light"> — no person assigned yet</span>
        </span>
        <span class="font-mono text-xs text-ink-tertiary">→</span>
      </button>

      <!-- Filter + sort. Past about thirty faces, scanning stops working. -->
      <div v-if="people.length" class="flex flex-wrap items-center gap-4">
        <input
          v-model="query"
          placeholder="Search people…"
          spellcheck="false"
          class="flex-1 min-w-[200px] bg-base border border-line rounded-sm px-3 py-2 text-[13px] text-ink"
        />
        <div class="flex items-center gap-2">
          <span class="label">Sort</span>
          <button
            v-for="s in SORTS"
            :key="s.id"
            class="font-mono text-[11px] transition-colors"
            :class="sort === s.id ? 'text-signal' : 'text-ink-tertiary hover:text-ink'"
            @click="setSort(s.id)"
          >{{ s.label }}</button>
        </div>
      </div>

      <div v-if="people.length === 0" class="flex flex-col items-center gap-2 py-16 text-center">
        <span class="signal-dot" style="width:10px;height:10px;background:var(--status-stopped)"></span>
        <div class="font-heading text-base font-semibold text-ink">No people detected</div>
        <div class="text-[13px] font-light text-ink-secondary">Scan a library to detect faces.</div>
      </div>

      <div
        v-else-if="visiblePeople.length === 0"
        class="font-mono text-xs text-ink-tertiary py-16 text-center"
      >nobody matches “{{ query }}”</div>

      <div v-else class="grid gap-4" style="grid-template-columns: repeat(auto-fill, minmax(148px, 1fr))">
        <button
          v-for="person in visiblePeople"
          :key="person.id"
          class="group flex flex-col text-left bg-surface border border-line rounded overflow-hidden transition-colors hover:border-line-strong"
          @mouseenter="onEnter(person)"
          @mouseleave="onLeave"
          @click="router.push({ name: 'person-detail', params: { id: person.id } })"
        >
          <span class="relative block aspect-square bg-raised">
            <!-- Absolute, not h-full: a percentage height against a box sized by
                 aspect-ratio resolves to auto, so every crop would stretch its
                 own tile to a different height and the wall would go ragged. -->
            <img
              v-if="cropUrl(person)"
              :src="cropUrl(person)"
              :alt="person.name || 'unnamed cluster'"
              class="absolute inset-0 w-full h-full object-cover"
              loading="lazy"
            />
            <span
              v-else
              class="absolute inset-0 flex items-center justify-center font-mono text-2xl text-ink-tertiary"
            >{{ (person.name || '?')[0] }}</span>

            <!-- Work waiting on this person, without reading a column. -->
            <span
              v-if="(person.pending_count || 0) > 0"
              class="absolute top-1.5 right-1.5 flex items-center gap-1 bg-base border border-line rounded-sm px-1.5 font-mono text-[11px]"
              style="color: var(--status-degraded)"
              :title="`${person.pending_count} shots pending review`"
            >
              <span class="signal-dot" style="width:5px;height:5px;background:var(--status-degraded)"></span>
              {{ person.pending_count }}
            </span>
          </span>

          <span class="px-2 py-1.5 border-t border-line">
            <span
              class="block text-[13px] font-medium truncate"
              :class="person.name ? 'text-ink' : 'text-ink-tertiary'"
            >{{ person.name || 'unnamed cluster' }}</span>
            <span class="block font-mono text-[11px] text-ink-tertiary">
              {{ person.shot_count || 0 }} · {{ person.face_count || 0 }}
            </span>
          </span>
        </button>
      </div>

      <div v-if="visiblePeople.length" class="font-mono text-[11px] text-ink-tertiary">
        shots · faces per person<span v-if="query"> · {{ visiblePeople.length }} of {{ people.length }} shown</span>
      </div>
    </template>
  </div>
</template>

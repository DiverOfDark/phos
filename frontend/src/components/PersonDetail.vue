<script setup>
import { ref, computed, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import ShotCard from '@/components/ShotCard.vue'

const route = useRoute()
const router = useRouter()

const personId = computed(() => route.params.id)

// Person data
const person = ref(null)
const shots = ref([])
const loading = ref(true)
const error = ref(null)

// Editable name
const isEditingName = ref(false)
const editName = ref('')
const savingName = ref(false)

// Delete confirmation
const showDeleteDialog = ref(false)
const deleting = ref(false)

// Merge dialog
const showMergeDialog = ref(false)
const allPeople = ref([])
const mergeFilter = ref('')
const merging = ref(false)

const displayName = computed(() => person.value?.name || 'Unnamed')

const filteredMergePeople = computed(() => {
  const q = mergeFilter.value.toLowerCase()
  // Exclude current person from merge targets
  const others = allPeople.value.filter((p) => p.id !== personId.value)
  if (!q) return others
  return others.filter(
    (p) => (p.name && p.name.toLowerCase().includes(q)) || p.id.toLowerCase().includes(q)
  )
})

async function fetchPerson() {
  try {
    const res = await fetch('/api/people')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const people = await res.json()
    person.value = people.find((p) => p.id === personId.value) || null
    allPeople.value = people
  } catch (e) {
    console.error('Failed to fetch person:', e)
    error.value = e.message
  }
}

async function fetchShots() {
  try {
    const res = await fetch(`/api/shots?person_id=${encodeURIComponent(personId.value)}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    shots.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch shots:', e)
    error.value = e.message
  }
}

async function loadData() {
  loading.value = true
  error.value = null
  try {
    await Promise.all([fetchPerson(), fetchShots()])
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

function startEditName() {
  editName.value = person.value?.name || ''
  isEditingName.value = true
}

function cancelEditName() {
  isEditingName.value = false
  editName.value = ''
}

async function saveName() {
  const newName = editName.value.trim()
  if (!newName || !person.value) return

  savingName.value = true
  try {
    const res = await fetch(`/api/people/${personId.value}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: newName }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    person.value = { ...person.value, name: newName }
    isEditingName.value = false
  } catch (e) {
    console.error('Failed to rename person:', e)
  } finally {
    savingName.value = false
  }
}

async function mergeWith(targetId) {
  if (!personId.value || merging.value) return

  merging.value = true
  try {
    const res = await fetch('/api/people/merge', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        source_id: personId.value,
        target_id: targetId,
      }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    // After merging, navigate to the target person
    showMergeDialog.value = false
    router.push({ name: 'person-detail', params: { id: targetId } })
  } catch (e) {
    console.error('Failed to merge people:', e)
  } finally {
    merging.value = false
  }
}

async function deletePerson() {
  if (!personId.value || deleting.value) return

  deleting.value = true
  try {
    const res = await fetch(`/api/people/${personId.value}`, {
      method: 'DELETE',
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    showDeleteDialog.value = false
    router.push('/people')
  } catch (e) {
    console.error('Failed to delete person:', e)
  } finally {
    deleting.value = false
  }
}

function navigateToShot(shotId) {
  router.push({ name: 'shot-detail', params: { id: shotId } })
}

// Re-fetch when route param changes (navigating between people)
watch(personId, () => {
  loadData()
})

onMounted(loadData)

defineExpose({ loadData, fetchPeople: fetchPerson, fetchShots })
</script>

<template>
  <div class="p-4 md:p-8 max-w-[1040px] w-full mx-auto flex flex-col gap-6">
    <div v-if="loading" class="font-mono text-xs text-ink-tertiary py-16 text-center">loading person…</div>

    <div v-else-if="error && !person" class="flex flex-col items-center gap-2 py-16 text-center">
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-error)"></span>
      <div class="font-heading text-base font-semibold text-ink">Could not load this person</div>
      <div class="font-mono text-xs text-error">{{ error }}</div>
      <button
        class="mt-2 border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
        @click="router.push('/people')"
      >Back to People</button>
    </div>

    <template v-else>
      <button
        class="self-start font-mono text-xs text-ink-tertiary hover:text-signal transition-colors"
        @click="router.push('/people')"
      >← People</button>

      <div class="flex flex-wrap items-center gap-4">
        <div class="w-16 h-16 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[22px] text-ink-tertiary shrink-0">
          <img v-if="person?.thumbnail_url" :src="person.thumbnail_url" class="w-full h-full object-cover" />
          <template v-else>{{ displayName[0] }}</template>
        </div>

        <div class="flex-1 min-w-[200px]">
          <div v-if="isEditingName" class="flex gap-2 items-center flex-wrap">
            <input
              v-model="editName"
              spellcheck="false"
              class="bg-surface border border-line rounded-sm px-3 py-2 font-heading text-lg font-semibold text-ink w-60"
              @keydown.enter="saveName"
              @keydown.esc="cancelEditName"
            />
            <button
              class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
              :disabled="savingName"
              @click="saveName"
            >Save</button>
            <button class="font-mono text-xs text-ink-tertiary hover:text-signal" @click="cancelEditName">cancel</button>
          </div>
          <h2 v-else class="text-[22px] font-semibold">{{ displayName }}</h2>
          <div class="font-mono text-xs text-ink-tertiary mt-1">
            {{ person?.shot_count || 0 }} shots · {{ person?.face_count || 0 }} faces ·
            {{ (person?.pending_count || 0) > 0 ? `${person.pending_count} pending` : 'nothing pending' }}
          </div>
        </div>

        <div class="flex flex-wrap gap-2 items-center">
          <button
            class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
            @click="startEditName"
          >Rename</button>
          <button
            class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors whitespace-nowrap"
            @click="showMergeDialog = true; mergeFilter = ''"
          >Merge into…</button>
          <template v-if="showDeleteDialog">
            <button
              class="rounded px-4 py-2 text-[13px] font-medium text-ink whitespace-nowrap"
              style="background: var(--status-error)"
              :disabled="deleting"
              @click="deletePerson"
            >Delete person + face markers</button>
            <button class="font-mono text-xs text-ink-tertiary hover:text-signal" @click="showDeleteDialog = false">cancel</button>
          </template>
          <button
            v-else
            class="border border-line-strong rounded px-4 py-2 text-[13px] text-error transition-colors"
            @click="showDeleteDialog = true"
          >Delete</button>
        </div>
      </div>

      <div v-if="showDeleteDialog" class="font-mono text-xs" style="color: var(--status-degraded)">
        Removes all face markers of this person and the person record. Photos are not deleted.
      </div>

      <div v-if="shots.length" class="grid gap-4" style="grid-template-columns: repeat(auto-fill, minmax(160px, 1fr))">
        <ShotCard
          v-for="shot in shots"
          :key="shot.id"
          :shot="shot"
          @click="navigateToShot(shot.id)"
        />
      </div>
      <div v-else class="flex flex-col items-center gap-2 py-16 text-center">
        <span class="signal-dot" style="width:10px;height:10px;background:var(--status-stopped)"></span>
        <div class="font-heading text-base font-semibold text-ink">No shots filed here yet</div>
      </div>
    </template>

    <!-- Merge dialog -->
    <div
      v-if="showMergeDialog"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
      style="background: var(--scrim)"
      @click="showMergeDialog = false"
    >
      <div
        class="w-[400px] max-w-full max-h-[calc(100vh-64px)] bg-overlay border border-line-strong rounded shadow-lg flex flex-col overflow-hidden"
        @click.stop
      >
        <div class="flex items-center justify-between px-6 py-4 border-b border-line">
          <div class="font-heading text-base font-semibold text-ink">Merge {{ displayName }} into…</div>
          <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="showMergeDialog = false">✕</button>
        </div>
        <div class="px-6 py-4 flex flex-col gap-2 overflow-y-auto min-h-0">
          <div class="text-xs font-light text-ink-secondary">
            All faces move to the target person; this person is deleted.
          </div>
          <input
            v-model="mergeFilter"
            placeholder="Search people…"
            spellcheck="false"
            class="bg-base border border-line rounded-sm px-3 py-2 text-[13px] text-ink w-full"
          />
          <div class="flex flex-col gap-0.5">
            <button
              v-for="p in filteredMergePeople"
              :key="p.id"
              class="flex items-center gap-2 p-2 border border-line rounded hover:bg-raised transition-colors text-left"
              :disabled="merging"
              @click="mergeWith(p.id)"
            >
              <span class="w-6 h-6 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[11px] text-ink-tertiary shrink-0">
                <img v-if="p.thumbnail_url" :src="p.thumbnail_url" class="w-full h-full object-cover" />
                <template v-else>{{ (p.name || '?')[0] }}</template>
              </span>
              <span class="flex-1 text-[13px] text-ink truncate">{{ p.name || 'unnamed cluster' }}</span>
              <span class="font-mono text-[11px] text-ink-tertiary">{{ p.shot_count || 0 }} shots</span>
            </button>
            <p v-if="filteredMergePeople.length === 0" class="font-mono text-[11px] text-ink-tertiary text-center py-3">
              no matching people
            </p>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

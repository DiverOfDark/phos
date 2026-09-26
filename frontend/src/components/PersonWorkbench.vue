<script setup>
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

const route = useRoute()
const router = useRouter()
const people = ref([])
const notice = ref('')
const busy = ref(false)
const menu = ref(null)
const pickerQuery = ref('')

function makePane(id, key) {
  return reactive({ id, key, person: null, shots: [], loading: false, error: '', scale: Number(localStorage.getItem(`phos_workbench_scale_${key}`) || 300) })
}
const left = makePane(route.params.id, 'left')
const right = makePane(route.params.id, 'right')
const panes = [left, right]

const filteredPeople = computed(() => {
  const q = pickerQuery.value.trim().toLowerCase()
  return people.value.filter(p => !q || (p.name || '').toLowerCase().includes(q))
})

async function loadPeople() {
  const res = await fetch('/api/people')
  if (res.ok) people.value = await res.json()
}

async function loadPane(pane) {
  pane.loading = true
  pane.error = ''
  try {
    const res = await fetch(`/api/people/${encodeURIComponent(pane.id)}/browse`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    pane.person = data.person
    pane.shots = data.shots
  } catch (e) {
    pane.error = e.message
  } finally {
    pane.loading = false
  }
}

async function reload() {
  await Promise.all(panes.map(loadPane))
}

function setScale(pane) {
  localStorage.setItem(`phos_workbench_scale_${pane.key}`, pane.scale)
}

function drag(event, payload) {
  event.dataTransfer.effectAllowed = 'move'
  event.dataTransfer.setData('application/x-phos-workbench', JSON.stringify(payload))
}

function dragged(event) {
  try { return JSON.parse(event.dataTransfer.getData('application/x-phos-workbench')) } catch { return null }
}

async function request(url, options) {
  busy.value = true
  menu.value = null
  try {
    const res = await fetch(url, options)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json().catch(() => ({}))
    await Promise.all([reload(), loadPeople()])
    return data
  } catch (e) {
    notice.value = `Operation failed · ${e.message}`
    throw e
  } finally {
    busy.value = false
  }
}

async function dropOnShot(event, targetShot) {
  event.stopPropagation()
  const item = dragged(event)
  if (!item || busy.value || item.shotId === targetShot.id) return
  if (item.type === 'shot') {
    await request('/api/shots/merge', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ source_id: item.shotId, target_id: targetShot.id }),
    })
    notice.value = 'Shots merged'
  } else if (item.type === 'file') {
    await request('/api/shots/move-file', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file_id: item.fileId, source_shot_id: item.shotId, target_shot_id: targetShot.id }),
    })
    notice.value = 'Photo moved'
  }
}

async function dropOnPane(event, pane) {
  const item = dragged(event)
  if (!item || busy.value) return
  if (item.type === 'shot') {
    if (item.personId === pane.id) return
    await reassign(item.shotId, pane.id)
  } else if (item.type === 'file') {
    const data = await request(`/api/shots/${item.shotId}/split`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file_ids: [item.fileId] }),
    })
    if (pane.id !== item.personId && data.new_shot_id) await reassign(data.new_shot_id, pane.id)
    notice.value = 'Photo split into a new shot'
  }
}

async function reassign(shotId, personId) {
  await request(`/api/shots/${shotId}`, {
    method: 'PUT', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ primary_person_id: personId === 'unsorted' ? '' : personId }),
  })
  notice.value = personId === 'unsorted' ? 'Shot moved to Unsorted' : 'Shot reassigned'
}

async function makePrimary(pane, shot) {
  await request(`/api/people/${pane.id}/primary-shot`, {
    method: 'PUT', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ shot_id: shot.id }),
  })
  notice.value = 'Primary shot updated'
}

function openMenu(event, pane, shot) {
  event.preventDefault()
  menu.value = { x: Math.min(event.clientX, window.innerWidth - 280), y: Math.min(event.clientY, window.innerHeight - 330), pane, shot, choosing: false }
  pickerQuery.value = ''
}

function closeMenu(event) {
  if (!event.target.closest?.('[data-shot-menu]')) menu.value = null
}

watch(() => route.params.id, id => { left.id = id; right.id = id; reload() })
onMounted(async () => { document.addEventListener('pointerdown', closeMenu); await Promise.all([loadPeople(), reload()]) })
onBeforeUnmount(() => document.removeEventListener('pointerdown', closeMenu))
</script>

<template>
  <div class="h-full min-h-0 flex flex-col bg-base">
    <header class="flex flex-wrap items-center gap-3 px-4 py-3 border-b border-line flex-none">
      <button class="font-mono text-xs text-ink-tertiary hover:text-signal" @click="router.push(`/person/${route.params.id}`)">← Person</button>
      <h2 class="font-heading text-base font-semibold">Shot workbench</h2>
      <span class="text-xs text-ink-secondary font-light">Drag a photo into a shot, or drag a shot header to merge.</span>
      <span class="flex-1"></span>
      <span v-if="busy" class="font-mono text-[11px] text-building">WORKING…</span>
      <span v-else-if="notice" class="font-mono text-[11px] text-ready">{{ notice }}</span>
    </header>

    <main class="flex-1 min-h-0 grid grid-cols-1 md:grid-cols-2 divide-y md:divide-y-0 md:divide-x divide-line">
      <section v-for="pane in panes" :key="pane.key" class="min-w-0 min-h-0 flex flex-col">
        <div class="p-3 border-b border-line flex items-center gap-2 flex-none bg-base">
          <select v-model="pane.id" class="min-w-0 flex-1 bg-surface border border-line rounded-sm px-2 py-1.5 text-[13px] text-ink" @change="loadPane(pane)">
            <option value="unsorted">Unsorted</option>
            <option v-for="p in people" :key="p.id" :value="p.id">{{ p.name || 'Unnamed' }} · {{ p.shot_count }} shots</option>
          </select>
          <label class="font-mono text-[10px] text-ink-tertiary whitespace-nowrap">SCALE</label>
          <input v-model.number="pane.scale" type="range" min="140" max="720" step="20" class="w-20 accent-[var(--accent)]" @input="setScale(pane)" />
        </div>

        <div class="flex-1 min-h-0 overflow-y-auto p-3" @dragover.prevent @drop.prevent="dropOnPane($event, pane)">
          <div v-if="pane.loading" class="py-16 text-center font-mono text-xs text-ink-tertiary">loading shots…</div>
          <div v-else-if="pane.error" class="py-16 text-center font-mono text-xs text-error">{{ pane.error }}</div>
          <div v-else class="flex flex-col gap-4">
            <article
              v-for="shot in pane.shots" :key="shot.id"
              class="border rounded bg-surface overflow-hidden"
              :class="shot.is_primary ? 'border-signal' : 'border-line'"
              style="content-visibility:auto; contain-intrinsic-size:auto 420px"
              @dragover.prevent @drop.prevent="dropOnShot($event, shot)" @contextmenu="openMenu($event, pane, shot)"
            >
              <header
                draggable="true"
                class="flex items-center gap-2 px-3 py-2 border-b border-line cursor-grab active:cursor-grabbing"
                @dragstart="drag($event, { type: 'shot', shotId: shot.id, personId: pane.id })"
              >
                <span class="font-mono text-[10px] text-ink-tertiary">⠿</span>
                <span class="font-mono text-[11px] text-ink-secondary truncate">{{ shot.timestamp || shot.id }}</span>
                <span v-if="shot.is_primary" class="font-mono text-[9px] tracking-[.08em] text-signal">PRIMARY</span>
                <span class="flex-1"></span>
                <span class="font-mono text-[10px] text-ink-tertiary">{{ shot.files.length }} FILE{{ shot.files.length === 1 ? '' : 'S' }}</span>
                <button class="px-1 text-ink-tertiary hover:text-signal" aria-label="Shot actions" @click.stop="openMenu($event, pane, shot)">⋯</button>
              </header>
              <div class="p-2 flex flex-col items-center gap-2">
                <img
                  v-for="file in shot.files" :key="file.id"
                  :src="file.thumbnail_url" loading="lazy" draggable="true"
                  class="block w-full h-auto border border-line rounded-sm bg-base cursor-grab active:cursor-grabbing"
                  :style="{ maxWidth: `${pane.scale}px`, objectFit: 'contain' }"
                  @dragstart.stop="drag($event, { type: 'file', fileId: file.id, shotId: shot.id, personId: pane.id })"
                />
              </div>
            </article>
            <div v-if="!pane.shots.length" class="py-16 text-center text-xs text-ink-tertiary">No shots for this person</div>
            <div class="h-16 border border-dashed border-line rounded flex items-center justify-center font-mono text-[10px] text-ink-tertiary">DROP PHOTO HERE TO SPLIT INTO A NEW SHOT</div>
          </div>
        </div>
      </section>
    </main>

    <div
      v-if="menu" data-shot-menu
      class="fixed z-50 w-64 bg-overlay border border-line-strong rounded shadow-lg overflow-hidden"
      :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
    >
      <template v-if="!menu.choosing">
        <button class="menu-row" @click="router.push(`/shot/${menu.shot.id}`)">Open shot</button>
        <button v-if="menu.pane.id !== 'unsorted'" class="menu-row" :disabled="menu.shot.is_primary" @click="makePrimary(menu.pane, menu.shot)">Make primary</button>
        <button class="menu-row border-t border-line" @click="menu.choosing = true">Reassign to…</button>
        <button v-if="menu.pane.id !== 'unsorted'" class="menu-row text-degraded" @click="reassign(menu.shot.id, 'unsorted')">Move to Unsorted</button>
      </template>
      <template v-else>
        <div class="p-2 border-b border-line"><input v-model="pickerQuery" autofocus placeholder="Search people…" class="w-full bg-base border border-line rounded-sm px-2 py-1.5 text-[13px]" /></div>
        <div class="max-h-64 overflow-y-auto p-1">
          <button v-for="p in filteredPeople" :key="p.id" class="menu-row" @click="reassign(menu.shot.id, p.id)">{{ p.name || 'Unnamed' }}</button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.menu-row { display: block; width: 100%; padding: .625rem .75rem; text-align: left; font-size: 13px; color: var(--text-secondary); }
.menu-row:hover:not(:disabled) { background: var(--bg-raised); color: var(--text-primary); }
.menu-row:disabled { opacity: .45; }
</style>

<script setup>
/**
 * Shots lane — the review desk proper.
 *
 * Left: the shot on a stage, with face boxes you can click and a filmstrip of
 * the files that belong to it. Right: the decision. Everything that changes the
 * shot lives in the panel; the stage grows no buttons beyond the two it needs
 * (play a video, draw a face).
 */
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'

const emit = defineEmits(['changed'])

const route = useRoute()
const router = useRouter()

// --- Shot list ---
const shots = ref([])
const currentIndex = ref(0)
const loading = ref(true)
const error = ref(null)
const clearedThisSession = ref(0)

const statusFilter = computed(() => route.query.status || 'pending')

const currentShot = computed(() => shots.value[currentIndex.value] || null)
const totalShots = computed(() => shots.value.length)

async function fetchShots() {
  loading.value = true
  error.value = null
  try {
    const status = statusFilter.value === 'unsorted' ? 'unsorted' : 'pending'
    const res = await fetch(`/api/shots?status=${encodeURIComponent(status)}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    shots.value = await res.json()
    currentIndex.value = 0
  } catch (e) {
    error.value = e.message
    shots.value = []
  } finally {
    loading.value = false
  }
}

watch(() => route.query.status, () => {
  fetchShots()
})

// --- Shot detail ---
const detail = ref(null)
const loadingDetail = ref(false)
const similarCount = ref(0)

async function fetchShotDetail(id) {
  if (!id) {
    detail.value = null
    return
  }
  loadingDetail.value = true
  try {
    const res = await fetch(`/api/shots/${id}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    detail.value = await res.json()
  } catch (e) {
    console.warn('Failed to fetch shot detail:', e)
    detail.value = null
  } finally {
    loadingDetail.value = false
  }
  fetchSimilarCount(id)
  fetchShotSuggestions()
}

async function fetchSimilarCount(id) {
  similarCount.value = 0
  try {
    const res = await fetch(`/api/shots/${id}/similar`)
    if (!res.ok) return
    const data = await res.json()
    similarCount.value = Array.isArray(data) ? data.length : (data.shots?.length || 0)
  } catch { /* the near-duplicates block just stays hidden */ }
}

watch(currentShot, (shot) => {
  if (shot) {
    fetchShotDetail(shot.id)
  } else {
    detail.value = null
  }
}, { immediate: true })

// --- Files ---
const files = computed(() => detail.value?.files || [])
const originalFile = computed(() => files.value.find(f => f.is_original))
const mainFile = computed(() => originalFile.value || files.value[0] || null)
const mainFileIsVideo = computed(() => mainFile.value?.mime_type?.startsWith('video/'))
// Videos get their first frame as a large thumbnail so face overlays land on a
// static image instead of a moving one.
const mainFileMediaUrl = computed(() => {
  if (!mainFile.value) return ''
  return mainFileIsVideo.value
    ? `/api/files/${mainFile.value.id}/thumbnail?w=1280`
    : `/api/files/${mainFile.value.id}`
})

function baseName(path) {
  return path?.split('/').pop() || 'file'
}

const curFileName = computed(() => baseName(mainFile.value?.path))
const curDims = computed(() => {
  const w = mainFile.value?.width || detail.value?.width
  const h = mainFile.value?.height || detail.value?.height
  const ms = mainFile.value?.duration_ms
  const size = w && h ? `${w}×${h}` : ''
  if (!ms) return size
  const secs = Math.round(ms / 1000)
  const clock = `${String(Math.floor(secs / 60)).padStart(2, '0')}:${String(secs % 60).padStart(2, '0')}`
  return size ? `${size} · ${clock}` : clock
})

const statusTag = computed(() => {
  const s = detail.value?.review_status || currentShot.value?.review_status
  if (s === 'confirmed') return { label: 'CONFIRMED', color: 'var(--status-ready)' }
  if (s === 'unsorted') return { label: 'UNSORTED', color: 'var(--status-pending)' }
  return { label: 'PENDING', color: 'var(--status-degraded)' }
})

// --- Video playback ---
const playing = ref(false)
watch(currentIndex, () => { playing.value = false })

// --- People ---
const people = ref([])
const peopleLoaded = ref(false)

async function fetchPeople() {
  if (peopleLoaded.value) return
  try {
    const res = await fetch('/api/people')
    if (res.ok) {
      people.value = await res.json()
      peopleLoaded.value = true
    }
  } catch (e) {
    console.warn('Failed to fetch people:', e)
  }
}

const peopleMap = computed(() => {
  const map = {}
  for (const p of people.value) map[p.id] = p
  return map
})

function personName(personId) {
  if (!personId) return null
  return peopleMap.value[personId]?.name || null
}

const assignName = computed(() => detail.value?.primary_person_name || 'Unsorted')
const assignInitial = computed(() => (detail.value?.primary_person_name || '?')[0])

// --- Face overlays ---
const faces = computed(() => detail.value?.faces || [])
const naturalWidth = ref(0)
const naturalHeight = ref(0)

function onMainImageLoad(e) {
  naturalWidth.value = e.target.naturalWidth
  naturalHeight.value = e.target.naturalHeight
}

function faceStyle(face) {
  if (!naturalWidth.value || !naturalHeight.value) return { display: 'none' }
  return {
    left: `${(face.box_x1 / naturalWidth.value) * 100}%`,
    top: `${(face.box_y1 / naturalHeight.value) * 100}%`,
    width: `${((face.box_x2 - face.box_x1) / naturalWidth.value) * 100}%`,
    height: `${((face.box_y2 - face.box_y1) / naturalHeight.value) * 100}%`,
  }
}

function faceLabel(face) {
  return face.person_name || personName(face.person_id) || '?'
}

// --- Face panel (reassign / delete a single detection) ---
const activeFaceId = ref(null)
const faceSearch = ref('')
const faceActionLoading = ref(false)
const faceSuggestions = ref([])
const creatingPerson = ref(false)

const activeFace = computed(() => faces.value.find(f => f.id === activeFaceId.value) || null)

function openFacePanel(faceId) {
  activeFaceId.value = faceId
  faceSearch.value = ''
  fetchFaceSuggestions(faceId)
}

function closeFacePanel() {
  activeFaceId.value = null
  faceSearch.value = ''
  faceSuggestions.value = []
}

async function fetchFaceSuggestions(faceId) {
  try {
    const res = await fetch(`/api/faces/${faceId}/suggestions`)
    if (res.ok) faceSuggestions.value = await res.json()
  } catch (e) {
    console.warn('Failed to fetch face suggestions:', e)
  }
}

const facePanelPeople = computed(() => {
  const q = faceSearch.value.toLowerCase().trim()
  let list = people.value
  if (q) list = list.filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
  if (faceSuggestions.value.length > 0) {
    const distMap = {}
    for (const s of faceSuggestions.value) distMap[s.person_id] = s.distance
    list = [...list].sort((a, b) => (distMap[a.id] ?? 999) - (distMap[b.id] ?? 999))
  }
  return list.slice(0, 8)
})

const faceCreateVisible = computed(() =>
  faceSearch.value.trim().length > 0 &&
  !people.value.some(p => (p.name || '').toLowerCase() === faceSearch.value.trim().toLowerCase())
)

async function createPersonAndAssign(faceId) {
  const name = faceSearch.value.trim()
  if (!name || creatingPerson.value) return
  creatingPerson.value = true
  try {
    const res = await fetch('/api/people', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const created = await res.json()
    await reassignFace(faceId, created.id)
    peopleLoaded.value = false
    fetchPeople()
  } catch (e) {
    console.error('Failed to create person:', e)
  } finally {
    creatingPerson.value = false
  }
}

async function reassignFace(faceId, targetPersonId) {
  faceActionLoading.value = true
  try {
    const res = await fetch(`/api/faces/${faceId}/person`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ person_id: targetPersonId }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
    closeFacePanel()
  } catch (e) {
    console.error('Failed to reassign face:', e)
  } finally {
    faceActionLoading.value = false
  }
}

async function deleteFace(faceId) {
  faceActionLoading.value = true
  try {
    const res = await fetch(`/api/faces/${faceId}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
    closeFacePanel()
  } catch (e) {
    console.error('Failed to delete face:', e)
  } finally {
    faceActionLoading.value = false
  }
}

// --- Shot-level suggestions ---
//
// The API suggests people per face; the desk decides per shot, so the faces'
// suggestions are merged and the best distance for each person wins.
const shotSuggestions = ref([])

async function fetchShotSuggestions() {
  shotSuggestions.value = []
  const list = faces.value
  if (list.length === 0) return
  const best = new Map()
  for (const face of list.slice(0, 4)) {
    try {
      const res = await fetch(`/api/faces/${face.id}/suggestions`)
      if (!res.ok) continue
      for (const s of await res.json()) {
        const prev = best.get(s.person_id)
        if (!prev || s.distance < prev.distance) best.set(s.person_id, s)
      }
    } catch { /* a missing suggestion just means fewer shortcuts */ }
  }
  shotSuggestions.value = [...best.values()]
    .sort((a, b) => a.distance - b.distance)
    .slice(0, 3)
}

const bestDistance = computed(() => {
  const s = shotSuggestions.value.find(x => x.person_id === detail.value?.primary_person_id)
    || shotSuggestions.value[0]
  return s ? s.distance.toFixed(2) : '—'
})

// --- Route to anyone else ---
const routeSearch = ref('')
const routeMatches = computed(() => {
  const q = routeSearch.value.toLowerCase().trim()
  if (!q) return []
  return people.value
    .filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
    .slice(0, 5)
})

// --- Files: master + split ---
async function setOriginal(fileId) {
  try {
    const res = await fetch(`/api/files/${fileId}/set-original`, { method: 'PUT' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
  } catch (e) {
    console.error('Failed to set original:', e)
  }
}

const splitSelection = ref(new Set())
const splitting = ref(false)
const splitMsg = ref('')

function toggleSplitFile(fileId) {
  if (splitSelection.value.has(fileId)) {
    splitSelection.value.delete(fileId)
  } else {
    splitSelection.value.add(fileId)
  }
  // Set mutations are not reactive on their own.
  splitSelection.value = new Set(splitSelection.value)
}

function clearSplit() {
  splitSelection.value = new Set()
}

// Splitting every file out would leave an empty shot behind, so the button only
// appears once at least one file is left unselected.
const splitReady = computed(() =>
  splitSelection.value.size > 0 && splitSelection.value.size < files.value.length
)

async function confirmSplit() {
  if (!currentShot.value || !splitReady.value || splitting.value) return
  splitting.value = true
  try {
    const res = await fetch(`/api/shots/${currentShot.value.id}/split`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file_ids: [...splitSelection.value] }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    splitMsg.value = `split ${splitSelection.value.size} file(s) into a new shot`
    clearSplit()
    await fetchShotDetail(currentShot.value.id)
    emit('changed')
  } catch (e) {
    console.error('Failed to split shot:', e)
    splitMsg.value = ''
  } finally {
    splitting.value = false
  }
}

// --- Decisions ---
const confirming = ref(false)

async function confirmShot() {
  if (!currentShot.value || confirming.value) return
  confirming.value = true
  try {
    const res = await fetch('/api/shots/batch/confirm', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ shot_ids: [currentShot.value.id] }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    removeCurrentAndAdvance()
  } catch (e) {
    console.error('Failed to confirm shot:', e)
  } finally {
    confirming.value = false
  }
}

function removeCurrentAndAdvance() {
  const idx = currentIndex.value
  shots.value.splice(idx, 1)
  clearedThisSession.value++
  if (shots.value.length === 0) {
    detail.value = null
  } else if (idx >= shots.value.length) {
    currentIndex.value = shots.value.length - 1
  }
  emit('changed')
}

const reassigningShot = ref(false)

async function reassignShot(personId) {
  if (!currentShot.value || reassigningShot.value) return
  reassigningShot.value = true
  try {
    const res = await fetch(`/api/shots/${currentShot.value.id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ primary_person_id: personId }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    routeSearch.value = ''
    removeCurrentAndAdvance()
  } catch (e) {
    console.error('Failed to reassign shot:', e)
  } finally {
    reassigningShot.value = false
  }
}

async function markUnsorted() {
  if (!currentShot.value) return
  try {
    const res = await fetch(`/api/shots/${currentShot.value.id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ primary_person_id: '' }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    removeCurrentAndAdvance()
  } catch (e) {
    console.error('Failed to mark unsorted:', e)
  }
}

const deleting = ref(false)
const deleteArmed = ref(false)

async function deleteShot() {
  if (!currentShot.value || deleting.value) return
  if (!deleteArmed.value) {
    deleteArmed.value = true
    return
  }
  deleting.value = true
  try {
    const res = await fetch(`/api/shots/${currentShot.value.id}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    deleteArmed.value = false
    removeCurrentAndAdvance()
  } catch (e) {
    console.error('Failed to delete shot:', e)
  } finally {
    deleting.value = false
  }
}

// --- Draw a face box by hand ---
const drawMode = ref(false)
const drawStart = ref(null)
const drawCurrent = ref(null)
const addingFace = ref(false)
const stageEl = ref(null)
const imageEl = ref(null)

function toggleDrawMode() {
  drawMode.value = !drawMode.value
  drawStart.value = null
  drawCurrent.value = null
  if (drawMode.value) {
    closeFacePanel()
    clearSplit()
  }
}

function exitDrawMode() {
  drawMode.value = false
  drawStart.value = null
  drawCurrent.value = null
}

function onDrawMousedown(e) {
  if (!drawMode.value || !imageEl.value) return
  e.preventDefault()
  const rect = imageEl.value.getBoundingClientRect()
  drawStart.value = { x: e.clientX - rect.left, y: e.clientY - rect.top }
  drawCurrent.value = { ...drawStart.value }
}

function onDrawMousemove(e) {
  if (!drawMode.value || !drawStart.value || !imageEl.value) return
  const rect = imageEl.value.getBoundingClientRect()
  drawCurrent.value = {
    x: Math.max(0, Math.min(e.clientX - rect.left, rect.width)),
    y: Math.max(0, Math.min(e.clientY - rect.top, rect.height)),
  }
}

async function onDrawMouseup() {
  if (!drawMode.value || !drawStart.value || !drawCurrent.value || !imageEl.value || !mainFile.value) return
  if (addingFace.value) return

  const rect = imageEl.value.getBoundingClientRect()
  const scaleX = naturalWidth.value / rect.width
  const scaleY = naturalHeight.value / rect.height

  const x1 = Math.min(drawStart.value.x, drawCurrent.value.x) * scaleX
  const y1 = Math.min(drawStart.value.y, drawCurrent.value.y) * scaleY
  const x2 = Math.max(drawStart.value.x, drawCurrent.value.x) * scaleX
  const y2 = Math.max(drawStart.value.y, drawCurrent.value.y) * scaleY

  // Ignore tiny rectangles — those are stray clicks, not faces.
  if (x2 - x1 < 10 || y2 - y1 < 10) {
    drawStart.value = null
    drawCurrent.value = null
    return
  }

  addingFace.value = true
  try {
    const res = await fetch(`/api/files/${mainFile.value.id}/faces`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ box_x1: x1, box_y1: y1, box_x2: x2, box_y2: y2 }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
  } catch (e) {
    console.error('Failed to add manual face:', e)
  } finally {
    addingFace.value = false
    exitDrawMode()
  }
}

const drawRectStyle = computed(() => {
  if (!drawStart.value || !drawCurrent.value) return { display: 'none' }
  return {
    left: `${Math.min(drawStart.value.x, drawCurrent.value.x)}px`,
    top: `${Math.min(drawStart.value.y, drawCurrent.value.y)}px`,
    width: `${Math.abs(drawCurrent.value.x - drawStart.value.x)}px`,
    height: `${Math.abs(drawCurrent.value.y - drawStart.value.y)}px`,
  }
})

// --- Navigation ---
function prevShot() {
  if (currentIndex.value > 0) currentIndex.value--
}

function nextShot() {
  if (currentIndex.value < shots.value.length - 1) currentIndex.value++
}

function onKeydown(e) {
  if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return

  if (e.key === 'Escape') {
    if (drawMode.value) exitDrawMode()
    else if (activeFaceId.value) closeFacePanel()
    else if (splitSelection.value.size) clearSplit()
    else if (deleteArmed.value) deleteArmed.value = false
    return
  }

  if (activeFaceId.value || drawMode.value) return

  if (e.key === 'Enter') {
    e.preventDefault()
    confirmShot()
  } else if (e.key === 'u' || e.key === 'U') {
    e.preventDefault()
    markUnsorted()
  } else if (e.key === 'f' || e.key === 'F') {
    e.preventDefault()
    toggleDrawMode()
  } else if (e.key === 'ArrowLeft') {
    e.preventDefault()
    prevShot()
  } else if (e.key === 'ArrowRight') {
    e.preventDefault()
    nextShot()
  } else if (['1', '2', '3'].includes(e.key)) {
    const s = shotSuggestions.value[Number(e.key) - 1]
    if (s) {
      e.preventDefault()
      reassignShot(s.person_id)
    }
  }
}

onMounted(() => {
  fetchShots()
  fetchPeople()
  window.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
})

watch(currentIndex, () => {
  naturalWidth.value = 0
  naturalHeight.value = 0
  closeFacePanel()
  clearSplit()
  exitDrawMode()
  splitMsg.value = ''
  routeSearch.value = ''
  deleteArmed.value = false
})

defineExpose({ loadData: fetchShots })
</script>

<template>
  <div class="flex flex-col flex-1 min-h-0">
    <div v-if="loading" class="flex-1 flex items-center justify-center py-16">
      <span class="font-mono text-xs text-ink-tertiary">loading queue…</span>
    </div>

    <div v-else-if="error" class="flex-1 flex flex-col items-center justify-center gap-2 p-16 text-center">
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-error)"></span>
      <div class="font-heading text-base font-semibold text-ink">Could not load shots</div>
      <div class="font-mono text-xs text-error">{{ error }}</div>
      <button
        class="mt-2 border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
        @click="fetchShots"
      >Retry</button>
    </div>

    <div v-else-if="totalShots === 0" class="flex-1 flex flex-col items-center justify-center gap-2 p-16 text-center">
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-ready)"></span>
      <div class="font-heading text-base font-semibold text-ink">Queue clear</div>
      <div class="text-[13px] font-light text-ink-secondary max-w-md">
        <template v-if="clearedThisSession">{{ clearedThisSession }} shots reviewed this session. </template>
        <template v-if="statusFilter === 'unsorted'">Nothing is sitting unsorted.</template>
        <template v-else>New pending shots appear after a scan.</template>
      </div>
    </div>

    <div v-else-if="currentShot" class="flex-1 flex flex-col lg:flex-row min-h-0">
      <!-- Photo stage -->
      <div class="flex-1 min-w-0 flex flex-col gap-4 p-4 lg:pl-8 lg:pr-6 lg:py-6">
        <div
          ref="stageEl"
          class="relative bg-surface border border-line rounded overflow-hidden flex items-center justify-center select-none"
          style="aspect-ratio: 3/2"
          :style="{ cursor: drawMode ? 'crosshair' : 'default' }"
          @mousedown="onDrawMousedown"
          @mousemove="onDrawMousemove"
          @mouseup="onDrawMouseup"
        >
          <div v-if="loadingDetail" class="font-mono text-[13px] text-ink-tertiary">loading shot…</div>

          <template v-else-if="mainFile">
            <!-- The image sits in its own positioned box so face boxes are
                 measured against the rendered picture, not the letterboxing. -->
            <div class="relative max-w-full max-h-full">
              <img
                ref="imageEl"
                :src="mainFileMediaUrl"
                :alt="curFileName"
                class="max-w-full max-h-full block object-contain"
                style="max-height: calc(100vh - 340px)"
                draggable="false"
                @load="onMainImageLoad"
              />

              <template v-if="!drawMode && naturalWidth > 0">
                <button
                  v-for="face in faces"
                  :key="face.id"
                  class="absolute rounded-sm p-0 transition-colors"
                  :style="{
                    ...faceStyle(face),
                    border: `1px solid ${face.person_id ? 'oklch(100% 0 0 / 0.7)' : 'var(--accent)'}`,
                    background: activeFaceId === face.id ? 'oklch(80% 0.16 80 / 0.12)' : 'transparent',
                  }"
                  title="Reassign or delete this face"
                  @click.stop="openFacePanel(face.id)"
                >
                  <span
                    class="absolute left-0 font-mono text-[11px] whitespace-nowrap bg-base border border-line rounded-sm px-1.5"
                    style="top: calc(100% + 4px)"
                    :style="{ color: face.person_id ? 'var(--text-secondary)' : 'var(--accent)' }"
                  >{{ faceLabel(face) }}</span>
                </button>
              </template>

              <div
                v-if="drawMode && drawStart && drawCurrent"
                class="absolute rounded-sm pointer-events-none"
                style="border: 1px dashed var(--accent); background: oklch(80% 0.16 80 / 0.08)"
                :style="drawRectStyle"
              ></div>
            </div>

            <!-- Video playback replaces the stage entirely -->
            <div v-if="playing" class="absolute inset-0 bg-base flex items-center justify-center">
              <video
                :src="`/api/files/${mainFile.id}`"
                class="max-w-full max-h-full"
                controls
                autoplay
              ></video>
              <button
                class="absolute top-2 right-2 bg-base border border-line-strong rounded-sm px-2 py-0.5 font-mono text-[11px] text-ink-secondary hover:text-signal"
                @click="playing = false"
              >stop</button>
            </div>

            <!-- Stage tags -->
            <div class="absolute top-2 left-2 flex gap-2 flex-wrap">
              <span class="tag bg-base" :style="{ color: statusTag.color }">{{ statusTag.label }}</span>
              <span v-if="mainFileIsVideo" class="tag bg-base" style="color: var(--status-building)">Video</span>
              <span v-if="drawMode" class="tag bg-base" style="color: var(--accent); border-color: var(--accent-muted)">Draw face — drag on image</span>
            </div>

            <div class="absolute top-2 right-2 flex gap-2">
              <button
                v-if="mainFileIsVideo && !playing"
                class="bg-base border border-line-strong rounded-sm px-2 py-0.5 font-mono text-[11px] text-ink-secondary hover:text-signal transition-colors"
                @click.stop="playing = true"
              >▶ play</button>
              <button
                class="bg-base border border-line-strong rounded-sm px-2 py-0.5 font-mono text-[11px] text-ink-secondary hover:text-signal transition-colors"
                @click.stop="toggleDrawMode"
              >+ face <span class="text-ink-tertiary">F</span></button>
            </div>
          </template>
        </div>

        <!-- Filmstrip -->
        <div class="flex gap-2 items-center flex-none min-w-0 overflow-x-auto">
          <button
            v-for="file in files"
            :key="file.id"
            :title="baseName(file.path)"
            class="w-16 h-12 flex-none rounded-sm bg-raised border overflow-hidden relative p-0"
            :class="splitSelection.has(file.id)
              ? 'border-signal'
              : file.id === mainFile?.id ? 'border-line-strong' : 'border-line'"
            @click="toggleSplitFile(file.id)"
          >
            <img :src="`/api/files/${file.id}/thumbnail`" class="w-full h-full object-cover" loading="lazy" />
            <span v-if="file.is_original" class="absolute top-1 left-1 w-1.5 h-1.5 rounded-full bg-signal"></span>
            <span v-if="file.synthetic" class="absolute bottom-0.5 left-1 font-mono text-[9px] tracking-[0.08em] text-ink-tertiary">GEN</span>
            <span v-if="splitSelection.has(file.id)" class="absolute top-0.5 right-1 font-mono text-[11px] text-signal">✓</span>
          </button>
          <div class="flex-1"></div>
          <div class="font-mono text-xs text-ink-tertiary whitespace-nowrap flex-none">
            {{ currentIndex + 1 }} / {{ totalShots }} · {{ files.length }} file{{ files.length === 1 ? '' : 's' }}
          </div>
        </div>
      </div>

      <!-- Decision panel -->
      <div class="w-full lg:w-[336px] flex-none border-t lg:border-t-0 lg:border-l border-line flex flex-col overflow-y-auto">
        <div class="p-6 flex flex-col gap-6 flex-1">
          <!-- Face panel -->
          <div
            v-if="activeFace"
            class="flex flex-col gap-2 p-3 border rounded bg-surface"
            style="border-color: var(--accent-muted)"
          >
            <div class="flex items-center gap-2">
              <div class="label flex-1">Face — {{ faceLabel(activeFace) }}</div>
              <button class="font-mono text-[11px] text-error" @click="deleteFace(activeFace.id)">delete</button>
              <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="closeFacePanel">✕</button>
            </div>
            <input
              v-model="faceSearch"
              placeholder="Search or type a new name…"
              spellcheck="false"
              class="bg-base border border-line rounded-sm px-2 py-1.5 text-[13px] text-ink w-full"
              @keydown.enter="faceCreateVisible ? createPersonAndAssign(activeFace.id) : null"
            />
            <div class="flex flex-col gap-0.5">
              <button
                v-for="p in facePanelPeople"
                :key="p.id"
                class="flex items-center gap-2 px-2 py-1.5 border border-line rounded hover:bg-raised transition-colors text-left"
                :disabled="faceActionLoading"
                @click="reassignFace(activeFace.id, p.id)"
              >
                <span class="w-5 h-5 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[10px] text-ink-tertiary shrink-0">
                  <img v-if="p.thumbnail_url" :src="p.thumbnail_url" class="w-full h-full object-cover" />
                  <template v-else>{{ (p.name || '?')[0] }}</template>
                </span>
                <span class="text-[13px] text-ink truncate">{{ p.name || 'unnamed' }}</span>
                <span v-if="activeFace.person_id === p.id" class="ml-auto font-mono text-[10px] text-signal">current</span>
              </button>
              <button
                v-if="faceCreateVisible"
                class="flex items-center gap-2 px-2 py-1.5 border rounded text-[13px] text-signal hover:bg-raised transition-colors text-left"
                style="border-color: var(--accent-muted)"
                :disabled="creatingPerson"
                @click="createPersonAndAssign(activeFace.id)"
              >Create "{{ faceSearch.trim() }}"</button>
            </div>
          </div>

          <!-- Route to -->
          <div class="flex flex-col gap-2">
            <div class="label">Route to</div>
            <div class="flex items-center gap-2">
              <span class="w-8 h-8 rounded bg-raised border border-line flex items-center justify-center font-mono text-[13px] text-ink-secondary">
                {{ assignInitial }}
              </span>
              <div class="flex-1 min-w-0">
                <div class="text-sm font-medium text-ink truncate">{{ assignName }}</div>
                <div class="font-mono text-[11px] text-ink-tertiary">match distance {{ bestDistance }}</div>
              </div>
            </div>
          </div>

          <!-- Suggestions -->
          <div class="flex flex-col gap-2">
            <div class="label">Suggestions</div>
            <div v-if="shotSuggestions.length" class="flex flex-col gap-0.5">
              <button
                v-for="(s, i) in shotSuggestions"
                :key="s.person_id"
                class="flex items-center gap-2 p-2 border rounded hover:bg-raised transition-colors text-left"
                :class="s.person_id === detail?.primary_person_id ? 'border-signal bg-surface' : 'border-line'"
                @click="reassignShot(s.person_id)"
              >
                <kbd class="kbd-ab">{{ i + 1 }}</kbd>
                <span class="flex-1 text-[13px] text-ink truncate">{{ s.person_name || 'unnamed' }}</span>
                <span
                  class="font-mono text-[11px]"
                  :style="{ color: s.distance < 0.4 ? 'var(--status-ready)' : 'var(--text-tertiary)' }"
                >{{ s.distance.toFixed(2) }}</span>
              </button>
            </div>
            <div v-else class="font-mono text-[11px] text-ink-tertiary">no close matches</div>

            <input
              v-model="routeSearch"
              placeholder="Route to anyone else…"
              spellcheck="false"
              class="bg-base border border-line rounded-sm px-2 py-1.5 text-[13px] text-ink w-full"
            />
            <div v-if="routeMatches.length" class="flex flex-col gap-0.5">
              <button
                v-for="p in routeMatches"
                :key="p.id"
                class="flex items-center gap-2 px-2 py-1.5 border border-line rounded hover:bg-raised transition-colors text-left"
                @click="reassignShot(p.id)"
              >
                <span class="w-5 h-5 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[10px] text-ink-tertiary shrink-0">
                  <img v-if="p.thumbnail_url" :src="p.thumbnail_url" class="w-full h-full object-cover" />
                  <template v-else>{{ (p.name || '?')[0] }}</template>
                </span>
                <span class="text-[13px] text-ink truncate">{{ p.name || 'unnamed' }}</span>
              </button>
            </div>
          </div>

          <!-- Files -->
          <div class="flex flex-col gap-2">
            <div class="label">Files · {{ files.length }}</div>
            <div class="flex flex-col gap-0.5">
              <div
                v-for="file in files"
                :key="file.id"
                class="flex items-center gap-2 px-2 py-1.5 border rounded text-left"
                :class="splitSelection.has(file.id) ? 'border-signal bg-surface' : 'border-line'"
              >
                <button
                  class="w-3 h-3 flex-none border rounded-sm flex items-center justify-center text-[9px]"
                  :class="splitSelection.has(file.id)
                    ? 'bg-signal border-signal text-signal-fg'
                    : 'border-line-strong text-transparent'"
                  title="Select for split"
                  @click="toggleSplitFile(file.id)"
                >✓</button>
                <span class="flex-1 font-mono text-xs text-ink-secondary truncate">{{ baseName(file.path) }}</span>
                <span v-if="file.mime_type?.startsWith('video/')" class="font-mono text-[10px] tracking-[0.08em] text-building">VID</span>
                <!-- An attribute of the file, not a status: the label register, no colour. -->
                <span
                  v-if="file.synthetic"
                  class="font-mono text-[10px] tracking-[0.08em] text-ink-tertiary border border-line rounded-sm px-1"
                  title="Made by a workflow, not a camera. Kept out of face recognition."
                >GENERATED</span>
                <span
                  v-if="file.is_original"
                  class="font-mono text-[10px] tracking-[0.08em] text-signal border rounded-sm px-1"
                  style="border-color: var(--accent-muted)"
                >MASTER</span>
                <button
                  v-else
                  class="font-mono text-[10px] text-ink-tertiary hover:text-signal transition-colors"
                  @click="setOriginal(file.id)"
                >set master</button>
              </div>
            </div>
            <button
              v-if="splitReady"
              class="self-start border border-line-strong rounded px-3 py-1.5 text-xs text-ink-secondary hover:text-signal transition-colors disabled:opacity-50"
              :disabled="splitting"
              @click="confirmSplit"
            >Split {{ splitSelection.size }} into new shot</button>
            <div v-if="splitMsg" class="font-mono text-[11px] text-ready">{{ splitMsg }}</div>
          </div>

          <!-- Near-duplicates -->
          <div v-if="similarCount > 0" class="flex flex-col gap-2">
            <div class="label">Near-duplicates</div>
            <button
              class="flex items-center gap-2 p-2 border border-line rounded hover:bg-raised transition-colors text-left"
              @click="router.push({ path: '/review', query: { lane: 'duplicates' } })"
            >
              <span class="signal-dot signal-pulse" style="background: var(--status-degraded)"></span>
              <span class="flex-1 text-xs text-ink-secondary">{{ similarCount }} similar shots found</span>
              <span class="font-mono text-xs text-ink-tertiary">→</span>
            </button>
          </div>
        </div>

        <!-- Actions -->
        <div class="border-t border-line px-6 py-4 flex flex-col gap-2 flex-none sticky bottom-0 bg-base">
          <button
            class="flex items-center justify-center gap-2 bg-signal text-signal-fg rounded p-2.5 text-sm font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
            :disabled="confirming"
            @click="confirmShot"
          >
            Confirm — {{ assignName }}
            <kbd class="font-mono text-[10px] border rounded-sm px-1" style="border-color: oklch(15% 0.01 80 / .3)">⏎</kbd>
          </button>
          <div class="flex gap-2">
            <button
              class="flex-1 border border-line-strong rounded p-2 text-xs text-ink-secondary hover:text-signal transition-colors"
              @click="markUnsorted"
            >Unsorted <span class="font-mono text-[10px] text-ink-tertiary">U</span></button>
            <button
              class="flex-1 border border-line-strong rounded p-2 text-xs text-ink-secondary hover:text-signal transition-colors disabled:opacity-40"
              :disabled="currentIndex >= totalShots - 1"
              @click="nextShot"
            >Skip <span class="font-mono text-[10px] text-ink-tertiary">→</span></button>
            <button
              class="flex-1 border rounded p-2 text-xs transition-colors"
              :class="deleteArmed ? 'text-ink' : 'text-error border-line-strong'"
              :style="deleteArmed ? 'background: var(--status-error); border-color: var(--status-error)' : ''"
              :disabled="deleting"
              @click="deleteShot"
            >{{ deleteArmed ? 'Confirm' : 'Delete' }}</button>
          </div>
          <div v-if="deleteArmed" class="font-mono text-[11px]" style="color: var(--status-degraded)">
            Deletes the shot and every file in it. Can't be undone. Esc to cancel.
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

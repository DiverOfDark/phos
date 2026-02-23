<script setup>
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Check,
  ChevronLeft,
  ChevronRight,
  Star,
  Trash2,
  X,
  RefreshCw,
  Scissors,
  ArrowRightLeft,
  FolderOpen,
  AlertCircle,
  Plus,
} from 'lucide-vue-next'

const route = useRoute()
const router = useRouter()

// --- Shot list ---
const shots = ref([])
const currentIndex = ref(0)
const loading = ref(true)
const error = ref(null)

const statusFilter = computed(() => route.query.status || 'pending')

const currentShot = computed(() => shots.value[currentIndex.value] || null)
const totalShots = computed(() => shots.value.length)
const reviewedCount = computed(() => {
  // Shots before the current index are "reviewed" in this session
  return currentIndex.value
})

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

// Reload when route query changes
watch(() => route.query.status, () => {
  fetchShots()
})

// --- Shot detail ---
const detail = ref(null)
const loadingDetail = ref(false)

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

// --- People list (for reassignment) ---
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
  for (const p of people.value) {
    map[p.id] = p
  }
  return map
})

function personName(personId) {
  if (!personId) return null
  return peopleMap.value[personId]?.name || null
}

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
  const left = (face.box_x1 / naturalWidth.value) * 100
  const top = (face.box_y1 / naturalHeight.value) * 100
  const width = ((face.box_x2 - face.box_x1) / naturalWidth.value) * 100
  const height = ((face.box_y2 - face.box_y1) / naturalHeight.value) * 100
  return {
    left: `${left}%`,
    top: `${top}%`,
    width: `${width}%`,
    height: `${height}%`,
  }
}

// --- Face popover (reassign / delete) ---
const activeFaceId = ref(null)
const faceSearch = ref('')
const faceActionLoading = ref(false)
const faceSuggestions = ref([])
const loadingSuggestions = ref(false)
const newPersonName = ref('')
const creatingPerson = ref(false)

function openFacePopover(faceId) {
  activeFaceId.value = faceId
  faceSearch.value = ''
  newPersonName.value = ''
  fetchFaceSuggestions(faceId)
}

function closeFacePopover() {
  activeFaceId.value = null
  faceSearch.value = ''
  newPersonName.value = ''
  faceSuggestions.value = []
}

async function fetchFaceSuggestions(faceId) {
  loadingSuggestions.value = true
  try {
    const res = await fetch(`/api/faces/${faceId}/suggestions`)
    if (res.ok) {
      faceSuggestions.value = await res.json()
    }
  } catch (e) {
    console.warn('Failed to fetch face suggestions:', e)
  } finally {
    loadingSuggestions.value = false
  }
}

async function createPersonAndAssign(faceId) {
  const name = newPersonName.value.trim()
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
    // Reassign face to the newly created person
    await reassignFace(faceId, created.id)
    // Refresh people list
    peopleLoaded.value = false
    fetchPeople()
  } catch (e) {
    console.error('Failed to create person:', e)
  } finally {
    creatingPerson.value = false
  }
}

const filteredPeople = computed(() => {
  const q = faceSearch.value.toLowerCase().trim()
  let list = people.value
  if (q) {
    list = list.filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
  }
  // If we have suggestions, sort matching people by suggestion distance
  if (faceSuggestions.value.length > 0) {
    const distMap = {}
    for (const s of faceSuggestions.value) {
      distMap[s.person_id] = s.distance
    }
    list = [...list].sort((a, b) => {
      const da = distMap[a.id] ?? 999
      const db = distMap[b.id] ?? 999
      return da - db
    })
  }
  return list
})

// Suggested person IDs for highlighting
const suggestedPersonIds = computed(() => {
  return new Set(faceSuggestions.value.map(s => s.person_id))
})

async function reassignFace(faceId, targetPersonId) {
  faceActionLoading.value = true
  try {
    const res = await fetch(`/api/faces/${faceId}/person`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ person_id: targetPersonId }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    // Refresh detail to pick up cascaded changes
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
    closeFacePopover()
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
    closeFacePopover()
  } catch (e) {
    console.error('Failed to delete face:', e)
  } finally {
    faceActionLoading.value = false
  }
}

// --- Set original ---
async function setOriginal(fileId) {
  try {
    const res = await fetch(`/api/files/${fileId}/set-original`, { method: 'PUT' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
  } catch (e) {
    console.error('Failed to set original:', e)
  }
}

// --- Action: Confirm ---
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
    // Remove from list and advance
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
  if (shots.value.length === 0) {
    detail.value = null
  } else if (idx >= shots.value.length) {
    currentIndex.value = shots.value.length - 1
  }
  // currentIndex stays or the watcher re-fetches detail
}

// --- Action: Reassign shot ---
const showReassignDropdown = ref(false)
const reassignSearch = ref('')
const reassigningShot = ref(false)

function toggleReassignDropdown() {
  showReassignDropdown.value = !showReassignDropdown.value
  reassignSearch.value = ''
}

function closeReassignDropdown() {
  showReassignDropdown.value = false
  reassignSearch.value = ''
}

const filteredReassignPeople = computed(() => {
  const q = reassignSearch.value.toLowerCase().trim()
  let list = people.value
  if (q) {
    list = list.filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
  }
  return list
})

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
    closeReassignDropdown()
    removeCurrentAndAdvance()
  } catch (e) {
    console.error('Failed to reassign shot:', e)
  } finally {
    reassigningShot.value = false
  }
}

// --- Action: Mark unsorted ---
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

// --- Action: Split mode ---
const splitMode = ref(false)
const splitSelection = ref(new Set())
const splitting = ref(false)

function enterSplitMode() {
  if (files.value.length < 2) return // need at least 2 files to split
  splitMode.value = true
  splitSelection.value = new Set()
}

function exitSplitMode() {
  splitMode.value = false
  splitSelection.value = new Set()
}

function toggleSplitFile(fileId) {
  if (splitSelection.value.has(fileId)) {
    splitSelection.value.delete(fileId)
  } else {
    splitSelection.value.add(fileId)
  }
  // Trigger reactivity
  splitSelection.value = new Set(splitSelection.value)
}

async function confirmSplit() {
  if (!currentShot.value || splitSelection.value.size === 0 || splitting.value) return
  splitting.value = true
  try {
    const res = await fetch(`/api/shots/${currentShot.value.id}/split`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file_ids: [...splitSelection.value] }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    exitSplitMode()
    // Refresh the detail
    if (currentShot.value) await fetchShotDetail(currentShot.value.id)
  } catch (e) {
    console.error('Failed to split shot:', e)
  } finally {
    splitting.value = false
  }
}

// --- Action: Draw face mode ---
const drawFaceMode = ref(false)
const drawStart = ref(null)   // { x, y } in CSS pixels relative to image
const drawCurrent = ref(null) // { x, y } during drag
const addingFace = ref(false)
const imageEl = ref(null)

function enterDrawFaceMode() {
  if (splitMode.value) exitSplitMode()
  closeFacePopover()
  closeReassignDropdown()
  drawFaceMode.value = true
  drawStart.value = null
  drawCurrent.value = null
}

function exitDrawFaceMode() {
  drawFaceMode.value = false
  drawStart.value = null
  drawCurrent.value = null
}

function onDrawMousedown(e) {
  if (!drawFaceMode.value || !imageEl.value) return
  e.preventDefault()
  const rect = imageEl.value.getBoundingClientRect()
  drawStart.value = { x: e.clientX - rect.left, y: e.clientY - rect.top }
  drawCurrent.value = { ...drawStart.value }
}

function onDrawMousemove(e) {
  if (!drawFaceMode.value || !drawStart.value || !imageEl.value) return
  const rect = imageEl.value.getBoundingClientRect()
  drawCurrent.value = {
    x: Math.max(0, Math.min(e.clientX - rect.left, rect.width)),
    y: Math.max(0, Math.min(e.clientY - rect.top, rect.height)),
  }
}

async function onDrawMouseup() {
  if (!drawFaceMode.value || !drawStart.value || !drawCurrent.value || !imageEl.value || !mainFile.value) return
  if (addingFace.value) return

  const rect = imageEl.value.getBoundingClientRect()
  const scaleX = naturalWidth.value / rect.width
  const scaleY = naturalHeight.value / rect.height

  // Convert CSS pixels to natural image pixels
  const x1 = Math.min(drawStart.value.x, drawCurrent.value.x) * scaleX
  const y1 = Math.min(drawStart.value.y, drawCurrent.value.y) * scaleY
  const x2 = Math.max(drawStart.value.x, drawCurrent.value.x) * scaleX
  const y2 = Math.max(drawStart.value.y, drawCurrent.value.y) * scaleY

  // Ignore tiny rectangles (likely accidental clicks)
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
    exitDrawFaceMode()
  }
}

const drawRectStyle = computed(() => {
  if (!drawStart.value || !drawCurrent.value) return { display: 'none' }
  const x1 = Math.min(drawStart.value.x, drawCurrent.value.x)
  const y1 = Math.min(drawStart.value.y, drawCurrent.value.y)
  const w = Math.abs(drawCurrent.value.x - drawStart.value.x)
  const h = Math.abs(drawCurrent.value.y - drawStart.value.y)
  return {
    left: `${x1}px`,
    top: `${y1}px`,
    width: `${w}px`,
    height: `${h}px`,
  }
})

// --- Navigation ---
function prevShot() {
  if (currentIndex.value > 0) {
    closeFacePopover()
    closeReassignDropdown()
    exitSplitMode()
    exitDrawFaceMode()
    currentIndex.value--
  }
}

function nextShot() {
  if (currentIndex.value < shots.value.length - 1) {
    closeFacePopover()
    closeReassignDropdown()
    exitSplitMode()
    exitDrawFaceMode()
    currentIndex.value++
  }
}

// --- Keyboard shortcuts ---
function onKeydown(e) {
  // Don't handle if focused on an input
  if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return

  if (e.key === 'Escape') {
    if (drawFaceMode.value) {
      exitDrawFaceMode()
    } else if (activeFaceId.value) {
      closeFacePopover()
    } else if (showReassignDropdown.value) {
      closeReassignDropdown()
    } else if (splitMode.value) {
      exitSplitMode()
    }
    return
  }

  // Don't handle other keys if a popover or draw mode is active
  if (activeFaceId.value || showReassignDropdown.value || drawFaceMode.value) return

  if (e.key === 'Enter') {
    e.preventDefault()
    confirmShot()
  } else if (e.key === 'r' || e.key === 'R') {
    e.preventDefault()
    toggleReassignDropdown()
  } else if (e.key === 's' || e.key === 'S') {
    e.preventDefault()
    if (splitMode.value) {
      confirmSplit()
    } else {
      enterSplitMode()
    }
  } else if (e.key === 'f' || e.key === 'F') {
    e.preventDefault()
    enterDrawFaceMode()
  } else if (e.key === 'ArrowLeft') {
    e.preventDefault()
    prevShot()
  } else if (e.key === 'ArrowRight') {
    e.preventDefault()
    nextShot()
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

// Reset zoom/face state when navigating
watch(currentIndex, () => {
  naturalWidth.value = 0
  naturalHeight.value = 0
  closeFacePopover()
  closeReassignDropdown()
  exitSplitMode()
  exitDrawFaceMode()
})

// Expose for App.vue refresh pattern
defineExpose({ loadData: fetchShots })
</script>

<template>
  <div>
    <!-- Header -->
    <div class="mb-6">
      <div class="flex items-center justify-between mb-2">
        <h2 class="text-2xl font-bold text-white">
          {{ statusFilter === 'unsorted' ? 'Unsorted Shots' : 'Review Queue' }}
        </h2>
        <div v-if="totalShots > 0" class="flex items-center gap-2 text-sm text-zinc-400">
          <span class="font-medium text-white">{{ currentIndex + 1 }}</span>
          <span>of</span>
          <span class="font-medium text-white">{{ totalShots }}</span>
        </div>
      </div>
      <p class="text-zinc-400 text-sm">
        {{ statusFilter === 'unsorted'
          ? 'Review shots with no assigned person.'
          : 'Review AI-assigned shots and confirm or reassign them.' }}
      </p>
    </div>

    <!-- Progress bar -->
    <div v-if="totalShots > 0" class="mb-6">
      <div class="flex items-center justify-between mb-1.5">
        <span class="text-xs font-medium text-zinc-400">
          {{ reviewedCount }} of {{ totalShots + reviewedCount }} reviewed
        </span>
        <span class="text-xs text-zinc-500">
          {{ totalShots }} remaining
        </span>
      </div>
      <div class="h-1.5 bg-zinc-800 rounded-full overflow-hidden">
        <div
          class="h-full bg-indigo-600 rounded-full transition-all duration-500 ease-out"
          :style="{ width: totalShots + reviewedCount > 0 ? `${(reviewedCount / (totalShots + reviewedCount)) * 100}%` : '0%' }"
        />
      </div>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="flex items-center justify-center py-20">
      <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="flex flex-col items-center justify-center py-20 text-center">
      <AlertCircle class="w-12 h-12 text-red-500/40 mb-4" />
      <p class="text-white font-medium mb-2">Failed to load shots</p>
      <p class="text-zinc-500 text-sm mb-4">{{ error }}</p>
      <Button variant="outline" class="border-white/10" @click="fetchShots">
        <RefreshCw class="w-4 h-4 mr-2" />
        Retry
      </Button>
    </div>

    <!-- Empty state -->
    <div v-else-if="totalShots === 0" class="flex flex-col items-center justify-center py-20 text-center">
      <div class="w-16 h-16 rounded-2xl bg-zinc-800 border border-white/5 flex items-center justify-center mb-4">
        <Check class="w-8 h-8 text-emerald-500" />
      </div>
      <p class="text-white font-medium mb-2">All caught up!</p>
      <p class="text-zinc-500 text-sm max-w-sm">
        {{ statusFilter === 'unsorted'
          ? 'No unsorted shots to review.'
          : 'All shots have been reviewed. New pending shots will appear here after scanning.' }}
      </p>
      <Button
        variant="outline"
        class="mt-4 border-white/10 text-zinc-300"
        @click="router.push('/')"
      >
        Back to Dashboard
      </Button>
    </div>

    <!-- Main review view -->
    <div v-else-if="currentShot" class="space-y-4">
      <!-- Shot info bar -->
      <div class="flex items-center gap-3 text-sm">
        <div
          :class="cn(
            'w-2 h-2 rounded-full',
            currentShot.review_status === 'confirmed' ? 'bg-emerald-500' : 'bg-yellow-500'
          )"
        />
        <span class="text-zinc-400">
          {{ currentShot.review_status === 'confirmed' ? 'Confirmed' : 'Pending' }}
        </span>
        <span v-if="detail?.primary_person_name" class="text-zinc-300 font-medium">
          {{ detail.primary_person_name }}
        </span>
        <span v-else class="text-zinc-500 italic">Unsorted</span>
        <span v-if="files.length > 1" class="text-zinc-500">
          {{ files.length }} files
        </span>
      </div>

      <!-- Files display -->
      <div class="flex flex-col lg:flex-row gap-4">
        <!-- Main image with face overlays -->
        <div class="flex-1 min-w-0">
          <div class="relative inline-block w-full">
            <!-- Loading spinner while detail loads -->
            <div v-if="loadingDetail" class="aspect-video bg-zinc-900 rounded-lg flex items-center justify-center">
              <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
            </div>
            <template v-else-if="mainFile">
              <div class="flex justify-center bg-zinc-900 rounded-lg overflow-hidden">
              <div
                class="relative inline-block"
                :class="{ 'cursor-crosshair': drawFaceMode }"
                @mousedown="onDrawMousedown"
                @mousemove="onDrawMousemove"
                @mouseup="onDrawMouseup"
              >
                <img
                  ref="imageEl"
                  :src="`/api/files/${mainFile.id}`"
                  :alt="mainFile.path?.split('/').pop() || 'Shot'"
                  class="max-w-full max-h-[60vh] select-none block"
                  draggable="false"
                  @load="onMainImageLoad"
                />

                <!-- Original badge on main image -->
                <div
                  v-if="mainFile.is_original"
                  class="absolute top-3 left-3 flex items-center gap-1 px-2 py-1 bg-yellow-500/20 backdrop-blur-sm border border-yellow-500/30 rounded-lg"
                >
                  <Star class="w-3.5 h-3.5 text-yellow-400 fill-yellow-400" />
                  <span class="text-xs font-semibold text-yellow-300">Original</span>
                </div>

                <!-- Draw face rectangle preview -->
                <div
                  v-if="drawFaceMode && drawStart && drawCurrent"
                  class="absolute border-2 border-dashed border-indigo-400 bg-indigo-500/10 rounded-sm pointer-events-none"
                  :style="drawRectStyle"
                />

                <!-- Draw face mode indicator -->
                <div
                  v-if="drawFaceMode"
                  class="absolute top-3 right-3 flex items-center gap-1.5 px-2.5 py-1.5 bg-indigo-600/80 backdrop-blur-sm border border-indigo-400/30 rounded-lg"
                >
                  <Plus class="w-3.5 h-3.5 text-white" />
                  <span class="text-xs font-semibold text-white">Draw face</span>
                </div>

                <!-- Face overlays -->
                <template v-if="faces.length > 0 && naturalWidth > 0 && !drawFaceMode">
                  <div
                    v-for="face in faces"
                    :key="face.id"
                    class="absolute border-2 border-indigo-400/70 rounded-sm cursor-pointer hover:border-indigo-300 transition-colors group/face"
                    :style="faceStyle(face)"
                    @click.stop="openFacePopover(face.id)"
                  >
                    <!-- Person name label -->
                    <div class="absolute -bottom-6 left-1/2 -translate-x-1/2 whitespace-nowrap">
                      <span class="px-1.5 py-0.5 text-[10px] font-medium bg-black/70 text-zinc-200 rounded backdrop-blur-sm group-hover/face:bg-indigo-600/80 group-hover/face:text-white transition-colors">
                        {{ personName(face.person_id) || 'Unknown' }}
                      </span>
                    </div>

                    <!-- Face action popover -->
                    <div
                      v-if="activeFaceId === face.id"
                      class="absolute top-full left-1/2 -translate-x-1/2 mt-8 z-50"
                      @click.stop
                    >
                      <div class="w-64 bg-zinc-900 border border-white/10 rounded-xl shadow-2xl overflow-hidden">
                        <div class="p-2 border-b border-white/5">
                          <div class="flex items-center justify-between mb-1.5 px-1">
                            <span class="text-xs font-semibold text-zinc-400">Reassign to</span>
                            <div class="flex items-center gap-1">
                              <button
                                class="p-1 text-zinc-500 hover:text-red-400 transition-colors rounded"
                                title="Delete this face detection"
                                @click.stop="deleteFace(face.id)"
                              >
                                <Trash2 class="w-3.5 h-3.5" />
                              </button>
                              <button
                                class="text-zinc-500 hover:text-white transition-colors"
                                @click.stop="closeFacePopover"
                              >
                                <X class="w-3.5 h-3.5" />
                              </button>
                            </div>
                          </div>
                          <Input
                            v-model="faceSearch"
                            placeholder="Search or type new name..."
                            class="h-7 text-xs bg-zinc-800/50 border-white/5"
                            @click.stop
                            @keydown.enter.stop="faceSearch.trim() ? (newPersonName = faceSearch.trim(), createPersonAndAssign(face.id)) : null"
                          />
                        </div>
                        <ScrollArea class="max-h-56">
                          <div class="p-1">
                            <div v-if="faceActionLoading || creatingPerson" class="flex items-center justify-center py-4">
                              <RefreshCw class="w-4 h-4 text-indigo-400 animate-spin" />
                            </div>
                            <template v-else>
                              <!-- Suggestions header -->
                              <div v-if="faceSuggestions.length > 0 && !faceSearch" class="px-2 pt-1 pb-0.5">
                                <span class="text-[10px] font-semibold text-zinc-500 uppercase tracking-wider">Suggested</span>
                              </div>

                              <button
                                v-for="person in filteredPeople"
                                :key="person.id"
                                class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-left transition-colors"
                                :class="face.person_id === person.id
                                  ? 'bg-indigo-600/20 text-indigo-300'
                                  : suggestedPersonIds.has(person.id) && !faceSearch
                                    ? 'bg-emerald-600/10 text-zinc-200 hover:bg-emerald-600/20'
                                    : 'text-zinc-300 hover:bg-white/5 hover:text-white'"
                                @click.stop="reassignFace(face.id, person.id)"
                              >
                                <div class="w-6 h-6 rounded-full bg-zinc-800 border border-zinc-700 overflow-hidden flex items-center justify-center shrink-0">
                                  <img
                                    v-if="person.thumbnail_url"
                                    :src="person.thumbnail_url"
                                    class="w-full h-full object-cover"
                                  />
                                  <span v-else class="text-[10px] font-bold text-zinc-500">{{ (person.name || '?')[0] }}</span>
                                </div>
                                <span class="text-xs font-medium truncate">{{ person.name || 'Unnamed' }}</span>
                                <span v-if="face.person_id === person.id" class="ml-auto text-[10px] text-indigo-400">current</span>
                                <span v-else-if="suggestedPersonIds.has(person.id) && !faceSearch" class="ml-auto text-[10px] text-emerald-400">match</span>
                              </button>

                              <!-- Create new person option (always visible when typing) -->
                              <div v-if="faceSearch.trim()" class="p-1 border-t border-white/5">
                                <button
                                  class="w-full flex items-center gap-2 px-2 py-2 rounded-lg text-left bg-indigo-600/10 text-indigo-300 hover:bg-indigo-600/20 transition-colors"
                                  @click.stop="newPersonName = faceSearch.trim(); createPersonAndAssign(face.id)"
                                >
                                  <div class="w-6 h-6 rounded-full bg-indigo-600/30 border border-indigo-500/30 flex items-center justify-center shrink-0">
                                    <span class="text-xs font-bold text-indigo-300">+</span>
                                  </div>
                                  <span class="text-xs font-medium">Create "{{ faceSearch.trim() }}"</span>
                                </button>
                              </div>

                              <p v-if="filteredPeople.length === 0 && !faceSearch.trim()" class="text-xs text-zinc-500 text-center py-3">No people found</p>
                            </template>
                          </div>
                        </ScrollArea>
                      </div>
                    </div>
                  </div>
                </template>
              </div>
              </div>
            </template>
          </div>
        </div>

        <!-- Side file list (when more than 1 file) -->
        <div v-if="files.length > 1" class="lg:w-48 flex lg:flex-col gap-2 overflow-x-auto lg:overflow-x-visible pb-2 lg:pb-0">
          <div
            v-for="file in files"
            :key="file.id"
            class="relative shrink-0 w-36 lg:w-full aspect-square bg-zinc-900 rounded-lg overflow-hidden border transition-colors cursor-pointer"
            :class="[
              splitMode
                ? splitSelection.has(file.id)
                  ? 'border-indigo-500 ring-2 ring-indigo-500/30'
                  : 'border-zinc-700 hover:border-zinc-600'
                : file.id === mainFile?.id
                  ? 'border-indigo-500'
                  : 'border-zinc-800 hover:border-zinc-700',
            ]"
            @click="splitMode ? toggleSplitFile(file.id) : null"
          >
            <img
              :src="`/api/files/${file.id}/thumbnail`"
              class="w-full h-full object-cover"
              loading="lazy"
            />

            <!-- Original badge -->
            <div
              v-if="file.is_original"
              class="absolute top-1.5 left-1.5 flex items-center gap-0.5 px-1.5 py-0.5 bg-yellow-500/20 backdrop-blur-sm border border-yellow-500/30 rounded"
            >
              <Star class="w-2.5 h-2.5 text-yellow-400 fill-yellow-400" />
              <span class="text-[9px] font-semibold text-yellow-300">Original</span>
            </div>

            <!-- Set as Original button (non-original files, not in split mode) -->
            <button
              v-if="!file.is_original && !splitMode"
              class="absolute bottom-1.5 left-1.5 right-1.5 flex items-center justify-center gap-1 py-1 bg-black/70 backdrop-blur-sm border border-white/10 rounded text-[10px] font-medium text-zinc-300 hover:text-white hover:bg-black/90 transition-colors opacity-0 group-hover:opacity-100"
              :class="{ 'opacity-100': true }"
              @click.stop="setOriginal(file.id)"
            >
              <Star class="w-2.5 h-2.5" />
              Set as Original
            </button>

            <!-- Split selection check -->
            <div
              v-if="splitMode && splitSelection.has(file.id)"
              class="absolute top-1.5 right-1.5 w-5 h-5 bg-indigo-600 rounded-full flex items-center justify-center"
            >
              <Check class="w-3 h-3 text-white" />
            </div>

            <!-- Filename -->
            <div class="absolute bottom-0 inset-x-0 px-1.5 py-1 bg-gradient-to-t from-black/70 to-transparent">
              <p class="text-[9px] text-zinc-300 truncate">{{ file.path?.split('/').pop() || 'file' }}</p>
            </div>
          </div>
        </div>
      </div>

      <!-- Navigation arrows (mobile-friendly) -->
      <div class="flex items-center justify-center gap-4 py-2">
        <button
          class="p-2 rounded-full transition-colors"
          :class="currentIndex > 0
            ? 'text-zinc-300 hover:text-white hover:bg-white/5'
            : 'text-zinc-700 cursor-not-allowed'"
          :disabled="currentIndex <= 0"
          @click="prevShot"
        >
          <ChevronLeft class="w-5 h-5" />
        </button>
        <span class="text-sm text-zinc-500 font-mono">{{ currentIndex + 1 }} / {{ totalShots }}</span>
        <button
          class="p-2 rounded-full transition-colors"
          :class="currentIndex < totalShots - 1
            ? 'text-zinc-300 hover:text-white hover:bg-white/5'
            : 'text-zinc-700 cursor-not-allowed'"
          :disabled="currentIndex >= totalShots - 1"
          @click="nextShot"
        >
          <ChevronRight class="w-5 h-5" />
        </button>
      </div>

      <!-- Action bar -->
      <div class="border-t border-white/5 pt-4">
        <!-- Split mode bar -->
        <div v-if="splitMode" class="flex items-center justify-between">
          <div class="flex items-center gap-2 text-sm">
            <Scissors class="w-4 h-4 text-indigo-400" />
            <span class="text-zinc-300">Split mode:</span>
            <span class="text-white font-medium">{{ splitSelection.size }} file{{ splitSelection.size !== 1 ? 's' : '' }} selected</span>
          </div>
          <div class="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              class="border-white/10 text-zinc-400 hover:text-white"
              @click="exitSplitMode"
            >
              Cancel
            </Button>
            <Button
              size="sm"
              class="bg-indigo-600 hover:bg-indigo-500 text-white"
              :disabled="splitSelection.size === 0 || splitting"
              @click="confirmSplit"
            >
              <RefreshCw v-if="splitting" class="w-3.5 h-3.5 animate-spin" />
              <Scissors v-else class="w-3.5 h-3.5" />
              Split (S)
            </Button>
          </div>
        </div>

        <!-- Normal action bar -->
        <div v-else class="flex flex-wrap items-center gap-2">
          <!-- Confirm -->
          <Button
            class="bg-emerald-600 hover:bg-emerald-500 text-white gap-2"
            :disabled="confirming"
            @click="confirmShot"
          >
            <RefreshCw v-if="confirming" class="w-4 h-4 animate-spin" />
            <Check v-else class="w-4 h-4" />
            Confirm
            <kbd class="ml-1 px-1.5 py-0.5 bg-white/10 rounded text-[10px] font-mono">Enter</kbd>
          </Button>

          <!-- Reassign -->
          <div class="relative">
            <Button
              variant="outline"
              class="border-white/10 text-zinc-300 hover:text-white gap-2"
              @click="toggleReassignDropdown"
            >
              <ArrowRightLeft class="w-4 h-4" />
              Reassign
              <kbd class="ml-1 px-1.5 py-0.5 bg-white/10 rounded text-[10px] font-mono">R</kbd>
            </Button>

            <!-- Reassign dropdown -->
            <div
              v-if="showReassignDropdown"
              class="absolute bottom-full left-0 mb-2 z-50"
              @click.stop
            >
              <div class="w-64 bg-zinc-900 border border-white/10 rounded-xl shadow-2xl overflow-hidden">
                <div class="p-2 border-b border-white/5">
                  <div class="flex items-center justify-between mb-1.5 px-1">
                    <span class="text-xs font-semibold text-zinc-400">Move shot to</span>
                    <button
                      class="text-zinc-500 hover:text-white transition-colors"
                      @click.stop="closeReassignDropdown"
                    >
                      <X class="w-3.5 h-3.5" />
                    </button>
                  </div>
                  <Input
                    v-model="reassignSearch"
                    placeholder="Search people..."
                    class="h-7 text-xs bg-zinc-800/50 border-white/5"
                    @click.stop
                  />
                </div>
                <ScrollArea class="max-h-56">
                  <div class="p-1">
                    <div v-if="reassigningShot" class="flex items-center justify-center py-4">
                      <RefreshCw class="w-4 h-4 text-indigo-400 animate-spin" />
                    </div>
                    <template v-else>
                      <button
                        v-for="person in filteredReassignPeople"
                        :key="person.id"
                        class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-left transition-colors"
                        :class="detail?.primary_person_id === person.id
                          ? 'bg-indigo-600/20 text-indigo-300'
                          : 'text-zinc-300 hover:bg-white/5 hover:text-white'"
                        @click.stop="reassignShot(person.id)"
                      >
                        <div class="w-6 h-6 rounded-full bg-zinc-800 border border-zinc-700 overflow-hidden flex items-center justify-center shrink-0">
                          <img
                            v-if="person.thumbnail_url"
                            :src="person.thumbnail_url"
                            class="w-full h-full object-cover"
                          />
                          <span v-else class="text-[10px] font-bold text-zinc-500">{{ (person.name || '?')[0] }}</span>
                        </div>
                        <span class="text-xs font-medium truncate">{{ person.name || 'Unnamed' }}</span>
                        <span v-if="detail?.primary_person_id === person.id" class="ml-auto text-[10px] text-indigo-400">current</span>
                      </button>
                      <p v-if="filteredReassignPeople.length === 0" class="text-xs text-zinc-500 text-center py-3">No people found</p>
                    </template>
                  </div>
                </ScrollArea>
              </div>
            </div>
          </div>

          <!-- Split -->
          <Button
            variant="outline"
            class="border-white/10 text-zinc-300 hover:text-white gap-2"
            :disabled="files.length < 2"
            :title="files.length < 2 ? 'Need at least 2 files to split' : 'Split selected files into a new shot'"
            @click="enterSplitMode"
          >
            <Scissors class="w-4 h-4" />
            Split
            <kbd class="ml-1 px-1.5 py-0.5 bg-white/10 rounded text-[10px] font-mono">S</kbd>
          </Button>

          <!-- Add Face -->
          <Button
            variant="outline"
            class="border-white/10 text-zinc-300 hover:text-white gap-2"
            title="Draw a face bounding box manually"
            @click="enterDrawFaceMode"
          >
            <Plus class="w-4 h-4" />
            Add Face
            <kbd class="ml-1 px-1.5 py-0.5 bg-white/10 rounded text-[10px] font-mono">F</kbd>
          </Button>

          <!-- Mark Unsorted -->
          <Button
            variant="outline"
            class="border-white/10 text-zinc-400 hover:text-zinc-200 gap-2"
            @click="markUnsorted"
          >
            <FolderOpen class="w-4 h-4" />
            Mark Unsorted
          </Button>

          <!-- Keyboard hints -->
          <div class="hidden md:flex items-center gap-3 ml-auto text-[10px] text-zinc-600">
            <span><kbd class="px-1 py-0.5 bg-zinc-800 border border-zinc-700 rounded font-mono">&#8592;</kbd> <kbd class="px-1 py-0.5 bg-zinc-800 border border-zinc-700 rounded font-mono">&#8594;</kbd> navigate</span>
            <span><kbd class="px-1 py-0.5 bg-zinc-800 border border-zinc-700 rounded font-mono">Esc</kbd> close</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

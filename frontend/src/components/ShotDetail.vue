<script setup>
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import EnhanceDialog from '@/components/EnhanceDialog.vue'

const route = useRoute()
const router = useRouter()

// --- Shot data ---
const shot = ref(null)
const loading = ref(true)
const error = ref('')

// --- Selected file in filmstrip ---
const selectedFileIndex = ref(0)
const videoPlaying = ref(false)

// --- People list for reassign dropdowns ---
const people = ref([])
const peopleLoaded = ref(false)

// --- Face reassign state ---
const reassignFaceId = ref(null)
const reassignSearch = ref('')
const reassigning = ref(false)

// --- Shot reassign dropdown state ---
const showReassignDropdown = ref(false)
const reassignShotSearch = ref('')

// --- Split mode ---
const splitMode = ref(false)
const splitSelection = ref(new Set())

// --- Delete confirmation ---
const showDeleteDialog = ref(false)
const deleting = ref(false)

// --- Delete file copy ---
const confirmDeleteFile = ref(false)
const deletingFile = ref(false)

// --- Similar shots ---
const similarShots = ref([]) // Array<{person_id, person_name, shots: SimilarShotItem[]}>
const showMergeConfirm = ref(false)
const mergeTargetShot = ref(null)
const mergeTargetPersonId = ref(null)
const merging = ref(false)

// --- ComfyUI enhance ---
const comfyuiAvailable = ref(false)
const showEnhanceDialog = ref(false)
const shotTasks = ref([])
let taskPollInterval = null

// Reset video playback state when switching files
watch(selectedFileIndex, () => { videoPlaying.value = false; confirmDeleteFile.value = false })

// --- Image natural dimensions (for face overlays) ---
const naturalWidth = ref(0)
const naturalHeight = ref(0)

// --- Computed ---
const shotId = computed(() => route.params.id)

const selectedFile = computed(() => {
  if (!shot.value?.files?.length) return null
  return shot.value.files[selectedFileIndex.value] || shot.value.files[0]
})

const selectedFileUrl = computed(() => {
  if (!selectedFile.value) return null
  return `/api/files/${selectedFile.value.id}`
})

const selectedFileThumbnailUrl = computed(() => {
  if (!selectedFile.value) return null
  return `/api/files/${selectedFile.value.id}/thumbnail`
})

const isVideo = computed(() => {
  const mime = selectedFile.value?.mime_type || ''
  return mime.startsWith('video/')
})

const selectedFilename = computed(() => {
  if (!selectedFile.value) return ''
  return selectedFile.value.path.split('/').pop()
})

const facesForSelectedFile = computed(() => {
  if (!shot.value?.faces?.length || !selectedFile.value) return []
  return shot.value.faces.filter(f => f.file_id === selectedFile.value.id)
})

const peopleMap = computed(() => {
  const map = {}
  for (const p of people.value) {
    map[p.id] = p
  }
  return map
})

const filteredPeople = computed(() => {
  const q = reassignSearch.value.toLowerCase().trim()
  let list = people.value
  if (q) {
    list = list.filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
  }
  return list
})

const filteredReassignShotPeople = computed(() => {
  const q = reassignShotSearch.value.toLowerCase().trim()
  let list = people.value
  if (q) {
    list = list.filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
  }
  return list
})

const statusTag = computed(() => {
  switch (shot.value?.review_status) {
    case 'confirmed': return { label: 'CONFIRMED', color: 'var(--status-ready)' }
    case 'unsorted': return { label: 'UNSORTED', color: 'var(--status-pending)' }
    default: return { label: 'PENDING', color: 'var(--status-degraded)' }
  }
})

/** Shot ids are long; the head shows the first 7 like a commit. */
const shotIdLabel = computed(() => `shot/${String(shotId.value || '').slice(0, 7)}`)

function baseName(path) {
  return path?.split('/').pop() || 'file'
}

/** The metadata block — only rows the backend actually knows. */
const metaRows = computed(() => {
  const s = shot.value
  if (!s) return []
  const rows = []
  if (s.timestamp) rows.push(['taken', new Date(s.timestamp).toLocaleString()])
  if (s.latitude != null && s.longitude != null) {
    rows.push(['gps', `${s.latitude.toFixed(6)}, ${s.longitude.toFixed(6)}`])
  }
  if (s.width && s.height) rows.push(['size', `${s.width}×${s.height}`])
  if (selectedFile.value?.file_size != null) rows.push(['bytes', formatFileSize(selectedFile.value.file_size)])
  if (s.folder_number != null) rows.push(['folder', String(s.folder_number).padStart(3, '0')])
  if (selectedFile.value?.path) rows.push(['path', selectedFile.value.path])
  return rows
})

/** Whether splitting the current selection would leave a non-empty shot behind. */
const shotSplitReady = computed(() =>
  splitSelection.value.size > 0 && splitSelection.value.size < (shot.value?.files?.length || 0)
)

function formatFileSize(bytes) {
  if (bytes == null) return null
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

// --- Metadata computed ---
const metadata = computed(() => {
  if (!shot.value) return []
  const items = []

  if (shot.value.timestamp) {
    items.push({
      label: 'Timestamp',
      value: new Date(shot.value.timestamp).toLocaleString(),
      icon: 'clock',
    })
  }

  if (shot.value.latitude != null && shot.value.longitude != null) {
    items.push({
      label: 'GPS',
      value: `${shot.value.latitude.toFixed(6)}, ${shot.value.longitude.toFixed(6)}`,
      icon: 'map',
    })
  }

  if (shot.value.width && shot.value.height) {
    items.push({
      label: 'Dimensions',
      value: `${shot.value.width} x ${shot.value.height}`,
      icon: 'size',
    })
  }

  if (shot.value.files?.length) {
    items.push({
      label: 'Files',
      value: `${shot.value.files.length} file${shot.value.files.length > 1 ? 's' : ''}`,
      icon: 'files',
    })
  }

  if (shot.value.folder_number != null) {
    items.push({
      label: 'Folder',
      value: String(shot.value.folder_number).padStart(3, '0'),
      icon: 'folder',
    })
  }

  if (shot.value.description) {
    items.push({
      label: 'Description',
      value: shot.value.description,
      icon: 'caption',
    })
  }

  return items
})

// --- Fetch shot data ---
async function fetchShot() {
  loading.value = true
  error.value = ''
  try {
    const res = await fetch(`/api/shots/${shotId.value}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    shot.value = await res.json()
    // Reset selected file index
    selectedFileIndex.value = 0
    // Reset image dimensions
    naturalWidth.value = 0
    naturalHeight.value = 0
  } catch (e) {
    console.error('Failed to fetch shot detail', e)
    error.value = 'Failed to load shot details.'
  } finally {
    loading.value = false
  }
}

// --- Fetch people ---
async function fetchPeople() {
  if (peopleLoaded.value) return
  try {
    const res = await fetch('/api/people')
    if (res.ok) {
      people.value = await res.json()
      peopleLoaded.value = true
    }
  } catch (e) {
    console.warn('Failed to fetch people', e)
  }
}

// --- Set original ---
async function setOriginal(fileId) {
  try {
    const res = await fetch(`/api/files/${fileId}/set-original`, { method: 'PUT' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    // Refresh shot data
    await fetchShot()
  } catch (e) {
    console.error('Failed to set original', e)
  }
}

async function deleteFileCopy(fileId) {
  deletingFile.value = true
  try {
    const res = await fetch(`/api/files/${fileId}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    confirmDeleteFile.value = false
    selectedFileIndex.value = 0
    await fetchShot()
  } catch (e) {
    console.error('Failed to delete file', e)
  } finally {
    deletingFile.value = false
  }
}

// --- Face overlay helpers ---
function onImageLoad(e) {
  naturalWidth.value = e.target.naturalWidth
  naturalHeight.value = e.target.naturalHeight
}

function faceStyle(face) {
  // For videos, face coords are from the original frame, so use shot dimensions
  const w = isVideo.value ? (shot.value?.width || naturalWidth.value) : naturalWidth.value
  const h = isVideo.value ? (shot.value?.height || naturalHeight.value) : naturalHeight.value
  if (!w || !h) return { display: 'none' }
  const left = (face.box_x1 / w) * 100
  const top = (face.box_y1 / h) * 100
  const width = ((face.box_x2 - face.box_x1) / w) * 100
  const height = ((face.box_y2 - face.box_y1) / h) * 100
  return {
    left: `${left}%`,
    top: `${top}%`,
    width: `${width}%`,
    height: `${height}%`,
  }
}

function personName(personId) {
  if (!personId) return null
  return peopleMap.value[personId]?.name || null
}

// --- Face reassign ---
function openReassign(faceId) {
  reassignFaceId.value = faceId
  reassignSearch.value = ''
}

function closeReassign() {
  reassignFaceId.value = null
  reassignSearch.value = ''
}

async function reassignFace(faceId, targetPersonId) {
  reassigning.value = true
  try {
    const res = await fetch(`/api/faces/${faceId}/person`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ person_id: targetPersonId }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    closeReassign()
    // Refresh shot data (primary person may have changed)
    await fetchShot()
  } catch (e) {
    console.error('Failed to reassign face', e)
  } finally {
    reassigning.value = false
  }
}

async function deleteFace(faceId) {
  try {
    const res = await fetch(`/api/faces/${faceId}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    closeReassign()
    await fetchShot()
  } catch (e) {
    console.error('Failed to delete face', e)
  }
}

// --- Shot reassign ---
function toggleReassignDropdown() {
  showReassignDropdown.value = !showReassignDropdown.value
  reassignShotSearch.value = ''
}

function closeReassignDropdown() {
  showReassignDropdown.value = false
  reassignShotSearch.value = ''
}

async function approveShot() {
  try {
    const res = await fetch(`/api/shots/${shotId.value}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ review_status: 'confirmed' }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchShot()
  } catch (e) {
    console.error('Failed to approve shot', e)
  }
}

async function reassignShot(personId) {
  try {
    const payload = {
      primary_person_id: personId || '',
      review_status: 'confirmed',
    }
    const res = await fetch(`/api/shots/${shotId.value}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    closeReassignDropdown()
    await fetchShot()
  } catch (e) {
    console.error('Failed to reassign shot', e)
  }
}

// --- Split ---
function enterSplitMode() {
  splitMode.value = true
  splitSelection.value = new Set()
}

function exitSplitMode() {
  splitMode.value = false
  splitSelection.value = new Set()
}

function toggleSplitFile(fileId) {
  const newSet = new Set(splitSelection.value)
  if (newSet.has(fileId)) {
    newSet.delete(fileId)
  } else {
    newSet.add(fileId)
  }
  splitSelection.value = newSet
}

async function confirmSplit() {
  if (splitSelection.value.size === 0) return
  // Cannot split ALL files
  if (splitSelection.value.size >= (shot.value?.files?.length || 0)) {
    return
  }
  try {
    const res = await fetch(`/api/shots/${shotId.value}/split`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file_ids: Array.from(splitSelection.value) }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    exitSplitMode()
    await fetchShot()
  } catch (e) {
    console.error('Failed to split shot', e)
  }
}

// --- Similar shots ---
async function fetchSimilarShots() {
  try {
    const res = await fetch(`/api/shots/${shotId.value}/similar`)
    if (res.ok) similarShots.value = await res.json()
  } catch (e) {
    console.warn('Failed to fetch similar shots', e)
  }
}

function openMergeConfirm(shot, personId = null) {
  mergeTargetShot.value = shot
  mergeTargetPersonId.value = personId
  showMergeConfirm.value = true
}

async function confirmMerge() {
  if (!mergeTargetShot.value) return
  merging.value = true
  try {
    const body = {
      source_id: mergeTargetShot.value.id,
      target_id: shotId.value,
    }
    if (mergeTargetPersonId.value) body.person_id = mergeTargetPersonId.value
    const res = await fetch('/api/shots/merge', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    showMergeConfirm.value = false
    mergeTargetShot.value = null
    mergeTargetPersonId.value = null
    await fetchShot()
    await fetchSimilarShots()
  } catch (e) {
    console.error('Failed to merge shot', e)
  } finally {
    merging.value = false
  }
}

// --- Delete shot ---
async function deleteShot() {
  deleting.value = true
  try {
    const res = await fetch(`/api/shots/${shotId.value}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    showDeleteDialog.value = false
    router.back()
  } catch (e) {
    console.error('Failed to delete shot', e)
  } finally {
    deleting.value = false
  }
}

// --- ComfyUI functions ---
async function checkComfyuiHealth() {
  try {
    const res = await fetch('/api/comfyui/health')
    if (!res.ok) throw new Error()
    const data = await res.json()
    comfyuiAvailable.value = data.status === 'ok'
  } catch {
    comfyuiAvailable.value = false
  }
}

async function fetchShotTasks() {
  if (!shotId.value) return
  try {
    const res = await fetch(`/api/comfyui/tasks?shot_id=${shotId.value}`)
    if (!res.ok) return
    const data = await res.json()
    shotTasks.value = data.items
  } catch {
    // ignore
  }
}

function startTaskPolling() {
  stopTaskPolling()
  taskPollInterval = setInterval(async () => {
    await fetchShotTasks()
    // Check if any task just completed - refetch shot for new files
    const hasActive = shotTasks.value.some(t => t.status === 'pending' || t.status === 'running')
    if (!hasActive) {
      stopTaskPolling()
      // Refetch shot data in case new files appeared
      await fetchShot()
    }
  }, 3000)
}

function stopTaskPolling() {
  if (taskPollInterval) {
    clearInterval(taskPollInterval)
    taskPollInterval = null
  }
}

function onTaskCreated(task) {
  fetchShotTasks()
  startTaskPolling()
}

async function retryTask(taskId) {
  try {
    const res = await fetch(`/api/comfyui/tasks/${taskId}/retry`, { method: 'POST' })
    if (!res.ok) throw new Error()
    await fetchShotTasks()
    startTaskPolling()
  } catch (e) {
    console.error('Failed to retry task', e)
  }
}

function taskStatusColor(status) {
  switch (status) {
    case 'completed': return 'var(--status-ready)'
    case 'failed': return 'var(--status-error)'
    case 'running': return 'var(--status-building)'
    case 'cancelled': return 'var(--status-stopped)'
    default: return 'var(--status-degraded)'
  }
}

// --- Navigation ---
function goBack() {
  router.back()
}

// --- Keyboard shortcuts ---
function onKeydown(e) {
  if (reassignFaceId.value || showReassignDropdown.value || showDeleteDialog.value || showMergeConfirm.value) {
    if (e.key === 'Escape') {
      closeReassign()
      closeReassignDropdown()
      showDeleteDialog.value = false
      showMergeConfirm.value = false
    }
    return
  }

  if (e.key === 'Escape') {
    if (splitMode.value) {
      exitSplitMode()
    } else {
      goBack()
    }
  }
}

// --- Close popovers when clicking outside ---
function onDocumentClick(e) {
  // Close reassign dropdown if clicking outside
  if (showReassignDropdown.value) {
    const dropdown = document.getElementById('reassign-dropdown')
    if (dropdown && !dropdown.contains(e.target)) {
      closeReassignDropdown()
    }
  }
}

// --- Lifecycle ---
onMounted(() => {
  fetchShot()
  fetchPeople()
  fetchSimilarShots()
  checkComfyuiHealth()
  fetchShotTasks()
  window.addEventListener('keydown', onKeydown)
  document.addEventListener('click', onDocumentClick)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
  document.removeEventListener('click', onDocumentClick)
  stopTaskPolling()
})

defineExpose({ loadData: fetchShot, fetchShots: fetchShot, fetchPeople })

// Refetch when route changes
watch(() => route.params.id, () => {
  if (route.params.id) {
    fetchShot()
    fetchSimilarShots()
    fetchShotTasks()
  }
})
</script>

<template>
  <div class="p-4 md:p-8 max-w-[1040px] w-full mx-auto flex flex-col gap-6">
    <div v-if="loading" class="font-mono text-xs text-ink-tertiary py-16 text-center">loading shot…</div>

    <div v-else-if="error" class="flex flex-col items-center gap-2 py-16 text-center">
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-error)"></span>
      <div class="font-heading text-base font-semibold text-ink">Could not load this shot</div>
      <div class="font-mono text-xs text-error">{{ error }}</div>
      <button
        class="mt-2 border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
        @click="goBack"
      >Back</button>
    </div>

    <template v-else-if="shot">
      <button class="self-start font-mono text-xs text-ink-tertiary hover:text-signal transition-colors" @click="goBack">← Back</button>

      <!-- Head -->
      <div class="flex items-center gap-4 flex-wrap">
        <div class="flex gap-1">
          <button
            class="border border-line-strong rounded px-2.5 py-1 font-mono text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-40"
            title="Previous shot"
            :disabled="!shot.prev_shot_id"
            @click="router.push(`/shot/${shot.prev_shot_id}`)"
          >‹</button>
          <button
            class="border border-line-strong rounded px-2.5 py-1 font-mono text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-40"
            title="Next shot"
            :disabled="!shot.next_shot_id"
            @click="router.push(`/shot/${shot.next_shot_id}`)"
          >›</button>
        </div>

        <h2 class="text-[22px] font-mono font-medium">{{ shotIdLabel }}</h2>
        <span class="tag" :style="{ color: statusTag.color }">{{ statusTag.label }}</span>
        <span class="font-mono text-xs text-ink-tertiary">
          {{ shot.primary_person_name || 'unsorted' }} · {{ shot.files?.length || 0 }} file{{ (shot.files?.length || 0) === 1 ? '' : 's' }}
        </span>

        <span class="flex-1"></span>

        <button
          v-if="shot.review_status !== 'confirmed'"
          class="border rounded px-4 py-2 text-[13px] whitespace-nowrap"
          style="border-color: var(--status-ready); color: var(--status-ready)"
          @click="approveShot"
        >Approve</button>

        <div class="relative" id="reassign-dropdown">
          <button
            class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors whitespace-nowrap"
            @click.stop="toggleReassignDropdown"
          >Reassign</button>
          <div
            v-if="showReassignDropdown"
            class="absolute right-0 z-40 w-64 bg-overlay border border-line-strong rounded shadow-lg p-2 flex flex-col gap-2"
            style="top: calc(100% + 8px)"
            @click.stop
          >
            <input
              v-model="reassignShotSearch"
              placeholder="Search people…"
              spellcheck="false"
              class="bg-base border border-line rounded-sm px-2 py-1.5 text-[13px] text-ink w-full"
            />
            <div class="flex flex-col gap-0.5 max-h-56 overflow-y-auto">
              <button
                v-for="p in filteredReassignShotPeople"
                :key="p.id"
                class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-raised transition-colors text-left"
                @click="reassignShot(p.id)"
              >
                <span class="w-5 h-5 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[10px] text-ink-tertiary shrink-0">
                  <img v-if="p.thumbnail_url" :src="p.thumbnail_url" class="w-full h-full object-cover" />
                  <template v-else>{{ (p.name || '?')[0] }}</template>
                </span>
                <span class="text-[13px] text-ink truncate">{{ p.name || 'unnamed' }}</span>
              </button>
              <button
                class="px-2 py-1.5 rounded text-left text-[13px] text-ink-tertiary hover:bg-raised hover:text-signal transition-colors"
                @click="reassignShot('')"
              >Leave unsorted</button>
            </div>
          </div>
        </div>

        <button
          v-if="(shot.files?.length || 0) > 1 && !splitMode"
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
          @click="enterSplitMode"
        >Split</button>

        <button
          v-if="comfyuiAvailable"
          class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors whitespace-nowrap"
          @click="showEnhanceDialog = true"
        >Enhance…</button>

        <template v-if="showDeleteDialog">
          <button
            class="rounded px-4 py-2 text-[13px] font-medium text-ink whitespace-nowrap"
            style="background: var(--status-error)"
            :disabled="deleting"
            @click="deleteShot"
          >Delete shot + files</button>
          <button class="font-mono text-xs text-ink-tertiary hover:text-signal" @click="showDeleteDialog = false">cancel</button>
        </template>
        <button
          v-else
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-error transition-colors"
          @click="showDeleteDialog = true"
        >Delete</button>
      </div>

      <!-- Split bar -->
      <div
        v-if="splitMode"
        class="flex items-center gap-4 px-4 py-2 border rounded bg-surface flex-wrap"
        style="border-color: var(--accent-muted)"
      >
        <span class="font-mono text-xs text-ink-secondary">
          split mode — select files below · {{ splitSelection.size }} selected
        </span>
        <span class="flex-1"></span>
        <button
          v-if="shotSplitReady"
          class="bg-signal text-signal-fg rounded px-4 py-1.5 text-xs font-medium hover:bg-signal-hover transition-colors"
          @click="confirmSplit"
        >Split into new shot</button>
        <button class="font-mono text-xs text-ink-tertiary hover:text-signal" @click="exitSplitMode">cancel</button>
      </div>

      <!-- Filmstrip -->
      <div v-if="(shot.files?.length || 0) > 1" class="flex gap-2 min-w-0 overflow-x-auto">
        <button
          v-for="(file, idx) in shot.files"
          :key="file.id"
          :title="baseName(file.path)"
          class="relative w-24 h-[72px] flex-none rounded-sm bg-raised border overflow-hidden p-0"
          :class="splitMode
            ? (splitSelection.has(file.id) ? 'border-signal' : 'border-line')
            : (idx === selectedFileIndex ? 'border-line-strong' : 'border-line')"
          @click="splitMode ? toggleSplitFile(file.id) : (selectedFileIndex = idx)"
        >
          <img :src="`/api/files/${file.id}/thumbnail`" class="w-full h-full object-cover" loading="lazy" />
          <span v-if="file.is_original" class="absolute top-1 left-1 w-1.5 h-1.5 rounded-full bg-signal"></span>
          <span v-if="file.mime_type?.startsWith('video/')" class="absolute bottom-0.5 right-1 font-mono text-[9px] text-building">VID</span>
          <span v-if="splitMode && splitSelection.has(file.id)" class="absolute top-1 right-1 font-mono text-[11px] text-signal">✓</span>
        </button>
      </div>

      <!-- Stage + side panel -->
      <div class="grid gap-6 items-start" style="grid-template-columns: minmax(0, 1fr)">
        <div class="grid gap-6 items-start" :class="'lg:grid-cols-[minmax(0,1fr)_320px]'">
          <div class="flex flex-col gap-2 min-w-0">
            <div class="relative bg-surface border border-line rounded overflow-hidden flex items-center justify-center" style="aspect-ratio: 3/2">
              <template v-if="selectedFile">
                <div class="relative max-w-full max-h-full">
                  <img
                    :src="isVideo ? `${selectedFileThumbnailUrl}?w=1280` : selectedFileUrl"
                    :alt="selectedFilename"
                    class="max-w-full max-h-full block object-contain"
                    style="max-height: 60vh"
                    @load="onImageLoad"
                  />
                  <button
                    v-for="face in facesForSelectedFile"
                    :key="face.id"
                    class="absolute rounded-sm p-0"
                    :style="{
                      ...faceStyle(face),
                      border: `1px solid ${face.person_id ? 'oklch(100% 0 0 / 0.7)' : 'var(--accent)'}`,
                      background: reassignFaceId === face.id ? 'oklch(80% 0.16 80 / 0.12)' : 'transparent',
                    }"
                    title="Reassign or delete this face"
                    @click.stop="openReassign(face.id)"
                  >
                    <span
                      class="absolute left-0 font-mono text-[11px] whitespace-nowrap bg-base border border-line rounded-sm px-1.5 text-ink-secondary"
                      style="top: calc(100% + 4px)"
                    >{{ face.person_name || personName(face.person_id) || '?' }}</span>
                  </button>
                </div>

                <div v-if="videoPlaying" class="absolute inset-0 bg-base flex items-center justify-center">
                  <video :src="selectedFileUrl" class="max-w-full max-h-full" controls autoplay></video>
                  <button
                    class="absolute top-2 right-2 bg-base border border-line-strong rounded-sm px-2 py-0.5 font-mono text-[11px] text-ink-secondary hover:text-signal"
                    @click="videoPlaying = false"
                  >stop</button>
                </div>

                <div v-if="isVideo && !videoPlaying" class="absolute top-2 right-2">
                  <button
                    class="bg-base border border-line-strong rounded-sm px-2 py-0.5 font-mono text-[11px] text-ink-secondary hover:text-signal transition-colors"
                    @click="videoPlaying = true"
                  >▶ play</button>
                </div>
              </template>
            </div>

            <div class="flex items-center gap-4 flex-wrap">
              <span class="font-mono text-xs text-ink-secondary truncate">{{ selectedFilename }}</span>
              <span v-if="selectedFile?.file_size != null" class="font-mono text-[11px] text-ink-tertiary whitespace-nowrap">
                {{ formatFileSize(selectedFile.file_size) }}
              </span>
              <span
                v-if="selectedFile?.is_original"
                class="font-mono text-[10px] tracking-[0.08em] text-signal border rounded-sm px-1"
                style="border-color: var(--accent-muted)"
              >MASTER</span>
              <span class="flex-1"></span>
              <template v-if="selectedFile && !selectedFile.is_original">
                <button
                  class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                  @click="setOriginal(selectedFile.id)"
                >set master</button>
                <template v-if="confirmDeleteFile">
                  <button
                    class="font-mono text-[11px] text-error"
                    :disabled="deletingFile"
                    @click="deleteFileCopy(selectedFile.id)"
                  >confirm delete</button>
                  <button class="font-mono text-[11px] text-ink-tertiary hover:text-signal" @click="confirmDeleteFile = false">cancel</button>
                </template>
                <button
                  v-else
                  class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                  @click="confirmDeleteFile = true"
                >delete copy</button>
              </template>
            </div>
          </div>

          <!-- Side panel -->
          <div class="flex flex-col gap-6 min-w-0">
            <!-- Faces -->
            <div class="flex flex-col gap-2">
              <div class="label">Faces</div>
              <div class="card-ab overflow-hidden">
                <div v-for="face in (shot.faces || [])" :key="face.id" class="border-b border-line">
                  <button
                    class="flex items-center gap-2 px-3 py-2 w-full hover:bg-raised transition-colors text-left"
                    @click="reassignFaceId === face.id ? closeReassign() : openReassign(face.id)"
                  >
                    <span
                      class="signal-dot"
                      :style="{ background: face.person_id ? 'var(--status-ready)' : 'var(--status-degraded)' }"
                    ></span>
                    <span class="flex-1 text-[13px] text-ink truncate">
                      {{ face.person_name || personName(face.person_id) || 'unknown' }}
                    </span>
                    <span class="font-mono text-[11px] text-ink-tertiary">{{ reassignFaceId === face.id ? '−' : '+' }}</span>
                  </button>
                  <div v-if="reassignFaceId === face.id" class="px-3 pb-3 pt-2 flex flex-col gap-2 bg-base">
                    <input
                      v-model="reassignSearch"
                      placeholder="Reassign to…"
                      spellcheck="false"
                      class="bg-surface border border-line rounded-sm px-2 py-1.5 text-[13px] text-ink w-full"
                    />
                    <div class="flex flex-col gap-0.5 max-h-48 overflow-y-auto">
                      <button
                        v-for="p in filteredPeople"
                        :key="p.id"
                        class="flex items-center gap-2 px-2 py-1.5 border border-line rounded hover:bg-raised transition-colors text-left"
                        :disabled="reassigning"
                        @click="reassignFace(face.id, p.id)"
                      >
                        <span class="w-5 h-5 rounded bg-raised border border-line overflow-hidden flex items-center justify-center font-mono text-[10px] text-ink-tertiary shrink-0">
                          <img v-if="p.thumbnail_url" :src="p.thumbnail_url" class="w-full h-full object-cover" />
                          <template v-else>{{ (p.name || '?')[0] }}</template>
                        </span>
                        <span class="text-[13px] text-ink truncate">{{ p.name || 'unnamed' }}</span>
                      </button>
                    </div>
                    <button class="self-start font-mono text-[11px] text-error" @click="deleteFace(face.id)">
                      delete face detection
                    </button>
                  </div>
                </div>
                <div v-if="!(shot.faces || []).length" class="px-3 py-3 font-mono text-[11px] text-ink-tertiary">
                  no faces detected
                </div>
              </div>
            </div>

            <!-- Also appears -->
            <div v-if="(shot.also_contains || []).length" class="flex flex-col gap-2">
              <div class="label">Also appears</div>
              <div class="flex flex-wrap gap-2">
                <button
                  v-for="p in shot.also_contains"
                  :key="p.id"
                  class="border border-line rounded px-3 py-1 text-[13px] text-ink-secondary hover:text-signal transition-colors"
                  @click="router.push({ name: 'person-detail', params: { id: p.id } })"
                >{{ p.name || 'unnamed' }}</button>
              </div>
            </div>

            <!-- Metadata -->
            <div class="flex flex-col gap-2">
              <div class="label">Metadata</div>
              <div class="grid font-mono text-xs" style="grid-template-columns: auto 1fr; gap: 4px 16px">
                <template v-for="row in metaRows" :key="row[0]">
                  <span class="text-ink-tertiary">{{ row[0] }}</span>
                  <span class="text-ink-secondary break-all">{{ row[1] }}</span>
                </template>
              </div>
              <div v-if="shot.description" class="text-xs font-light text-ink-secondary">"{{ shot.description }}"</div>
            </div>

            <!-- AI enhancements -->
            <div v-if="shotTasks.length" class="flex flex-col gap-2">
              <div class="label">AI enhancements</div>
              <div class="card-ab overflow-hidden">
                <div
                  v-for="task in shotTasks"
                  :key="task.id"
                  class="flex items-center gap-2 px-3 py-2 border-b border-line"
                >
                  <span
                    class="signal-dot"
                    :class="{ 'signal-pulse': task.status === 'running' || task.status === 'pending' }"
                    :style="{ background: taskStatusColor(task.status), width: '6px', height: '6px' }"
                  ></span>
                  <span class="flex-1 font-mono text-xs text-ink truncate">{{ task.workflow_name || task.workflow_id }}</span>
                  <span
                    class="font-mono text-[11px] tracking-[0.08em] uppercase"
                    :style="{ color: taskStatusColor(task.status) }"
                  >{{ task.status }}</span>
                  <button
                    v-if="task.status === 'failed'"
                    class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                    @click="retryTask(task.id)"
                  >retry</button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Near-duplicates -->
      <div v-if="similarShots.length" class="flex flex-col gap-4">
        <div v-for="group in similarShots" :key="group.person_id || 'none'" class="flex flex-col gap-2">
          <div class="label">Similar · {{ group.person_name || 'unsorted' }}</div>
          <div class="flex flex-wrap gap-4">
            <div v-for="sim in group.shots" :key="sim.id" class="flex flex-col gap-1 w-40">
              <button
                class="aspect-[4/3] bg-surface border border-line rounded overflow-hidden flex items-center justify-center p-0"
                @click="router.push({ name: 'shot-detail', params: { id: sim.id } })"
              >
                <img v-if="sim.thumbnail_url" :src="sim.thumbnail_url" class="w-full h-full object-cover" loading="lazy" />
                <span v-else class="font-mono text-[11px] text-ink-tertiary">no thumbnail</span>
              </button>
              <div class="flex justify-between items-center gap-2 whitespace-nowrap">
                <span class="font-mono text-[11px] text-ink-tertiary">{{ sim.file_count || 1 }} file(s)</span>
                <button
                  class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                  title="Merge this shot into the one above"
                  @click="openMergeConfirm(sim, group.person_id)"
                >merge here</button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>

    <!-- Merge confirmation -->
    <div
      v-if="showMergeConfirm"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
      style="background: var(--scrim)"
      @click="showMergeConfirm = false"
    >
      <div class="w-[400px] max-w-full bg-overlay border border-line-strong rounded shadow-lg flex flex-col overflow-hidden" @click.stop>
        <div class="flex items-center justify-between px-6 py-4 border-b border-line">
          <div class="font-heading text-base font-semibold text-ink">Merge into this shot</div>
          <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="showMergeConfirm = false">✕</button>
        </div>
        <div class="px-6 py-4 flex flex-col gap-4">
          <div class="text-[13px] font-light text-ink-secondary">
            Every file from the other shot moves here and the other shot record is removed.
            The files themselves are not deleted.
          </div>
          <div class="flex gap-2">
            <button
              class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
              :disabled="merging"
              @click="confirmMerge"
            >Merge</button>
            <button
              class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
              @click="showMergeConfirm = false"
            >Cancel</button>
          </div>
        </div>
      </div>
    </div>

    <EnhanceDialog
      v-model:open="showEnhanceDialog"
      :shot-id="shotId"
      :shot-label="shotIdLabel"
      @task-created="onTaskCreated"
    />
  </div>
</template>

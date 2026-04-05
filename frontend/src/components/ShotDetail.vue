<script setup>
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import ShotCard from '@/components/ShotCard.vue'
import EnhanceDialog from '@/components/EnhanceDialog.vue'
import {
  ArrowLeft,
  Star,
  Crown,
  Trash2,
  User,
  Users,
  Scissors,
  X,
  RefreshCw,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  MapPin,
  Clock,
  Maximize2,
  HardDrive,
  Check,
  FileImage,
  Film,
  Merge,
  Wand2,
  AlertCircle,
  RotateCcw,
  Play,
  MessageSquare,
} from 'lucide-vue-next'

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

const statusDot = computed(() => {
  switch (shot.value?.review_status) {
    case 'confirmed':
      return 'bg-emerald-500'
    case 'pending':
    default:
      return 'bg-yellow-500'
  }
})

const statusLabel = computed(() => {
  switch (shot.value?.review_status) {
    case 'confirmed':
      return 'Confirmed'
    case 'pending':
    default:
      return 'Pending'
  }
})

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

function taskStatusBadgeClass(status) {
  switch (status) {
    case 'completed': return 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20'
    case 'failed': return 'bg-red-500/10 text-red-400 border-red-500/20'
    case 'running': return 'bg-indigo-500/10 text-indigo-400 border-indigo-500/20'
    case 'cancelled': return 'bg-zinc-500/10 text-zinc-400 border-zinc-500/20'
    default: return 'bg-yellow-500/10 text-yellow-400 border-yellow-500/20'
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
  <div class="space-y-6">
    <!-- Header: Back + Title + Actions -->
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <Button
          variant="ghost"
          size="icon"
          class="text-zinc-400 hover:text-white rounded-xl hover:bg-white/5 shrink-0"
          @click="goBack"
        >
          <ArrowLeft class="w-5 h-5" />
        </Button>
        <div>
          <h2 class="text-xl font-bold text-white">Shot Detail</h2>
          <p v-if="shot" class="text-zinc-500 text-xs mt-0.5">
            {{ shot.files?.length || 0 }} file{{ (shot.files?.length || 0) !== 1 ? 's' : '' }}
          </p>
        </div>
        <div class="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            class="text-zinc-400 hover:text-white rounded-xl hover:bg-white/5"
            :disabled="!shot?.prev_shot_id"
            @click="router.push(`/shot/${shot.prev_shot_id}`)"
          >
            <ChevronLeft class="w-5 h-5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="text-zinc-400 hover:text-white rounded-xl hover:bg-white/5"
            :disabled="!shot?.next_shot_id"
            @click="router.push(`/shot/${shot.next_shot_id}`)"
          >
            <ChevronRight class="w-5 h-5" />
          </Button>
        </div>
      </div>

      <div class="flex items-center gap-2">
        <!-- Review status badge -->
        <div v-if="shot" class="flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-zinc-800/50 border border-white/5">
          <div :class="cn('w-2 h-2 rounded-full', statusDot)" />
          <span class="text-xs font-medium text-zinc-300">{{ statusLabel }}</span>
        </div>
        <Button
          v-if="shot?.review_status !== 'confirmed'"
          variant="ghost"
          size="sm"
          class="gap-1.5 text-emerald-400 hover:text-emerald-300 hover:bg-emerald-500/10"
          @click="approveShot"
        >
          <Check class="w-3.5 h-3.5" />
          Approve
        </Button>

        <!-- Enhance button (ComfyUI) -->
        <Button
          v-if="comfyuiAvailable && shot"
          variant="ghost"
          size="sm"
          class="gap-1.5 text-indigo-400 hover:text-indigo-300 hover:bg-indigo-500/10"
          @click="showEnhanceDialog = true"
        >
          <Wand2 class="w-3.5 h-3.5" />
          Enhance
        </Button>

        <!-- Split button -->
        <Button
          v-if="shot && shot.files?.length > 1 && !splitMode"
          variant="ghost"
          size="sm"
          class="gap-1.5 text-zinc-400 hover:text-white hover:bg-white/5"
          @click="enterSplitMode"
        >
          <Scissors class="w-3.5 h-3.5" />
          Split
        </Button>

        <!-- Delete button -->
        <Button
          variant="ghost"
          size="icon-sm"
          class="text-zinc-400 hover:text-red-400 hover:bg-red-500/10"
          @click="showDeleteDialog = true"
        >
          <Trash2 class="w-4 h-4" />
        </Button>
      </div>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="flex items-center justify-center py-20">
      <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="flex flex-col items-center justify-center py-20 text-center">
      <p class="text-red-400 text-sm">{{ error }}</p>
      <Button variant="ghost" class="mt-4 text-zinc-400" @click="fetchShot">Retry</Button>
    </div>

    <!-- Content -->
    <template v-else-if="shot">
      <!-- Primary Person Indicator + Reassign -->
      <div class="flex items-center gap-3 p-3 rounded-xl bg-zinc-800/30 border border-white/5">
        <div class="flex items-center gap-2 flex-1 min-w-0">
          <div class="w-8 h-8 rounded-full bg-zinc-800 border border-zinc-700 overflow-hidden flex items-center justify-center shrink-0">
            <Users v-if="!shot.primary_person_id" class="w-4 h-4 text-zinc-500" />
            <span v-else class="text-xs font-bold text-zinc-400">
              {{ (shot.primary_person_name || '?')[0] }}
            </span>
          </div>
          <div class="min-w-0">
            <p class="text-sm font-medium text-white truncate">
              {{ shot.primary_person_name || 'Unsorted' }}
            </p>
            <p class="text-[10px] text-zinc-500">Primary person</p>
          </div>
        </div>

        <!-- Reassign dropdown -->
        <div class="relative" id="reassign-dropdown">
          <Button
            variant="ghost"
            size="sm"
            class="gap-1.5 text-zinc-400 hover:text-white hover:bg-white/5"
            @click.stop="toggleReassignDropdown"
          >
            <RefreshCw class="w-3.5 h-3.5" />
            Reassign
            <ChevronDown class="w-3 h-3" />
          </Button>

          <div
            v-if="showReassignDropdown"
            class="absolute right-0 top-full mt-2 z-50"
            @click.stop
          >
            <div class="w-60 bg-zinc-900 border border-white/10 rounded-xl shadow-2xl overflow-hidden">
              <div class="p-2 border-b border-white/5">
                <div class="flex items-center justify-between mb-1.5 px-1">
                  <span class="text-xs font-semibold text-zinc-400">Move to person</span>
                  <button
                    class="text-zinc-500 hover:text-white transition-colors"
                    @click.stop="closeReassignDropdown"
                  >
                    <X class="w-3.5 h-3.5" />
                  </button>
                </div>
                <Input
                  v-model="reassignShotSearch"
                  placeholder="Search people..."
                  class="h-7 text-xs bg-zinc-800/50 border-white/5"
                  @click.stop
                />
              </div>
              <ScrollArea class="max-h-56">
                <div class="p-1">
                  <!-- Unsorted option -->
                  <button
                    class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-left transition-colors"
                    :class="!shot.primary_person_id
                      ? 'bg-indigo-600/20 text-indigo-300'
                      : 'text-zinc-300 hover:bg-white/5 hover:text-white'"
                    @click.stop="reassignShot(null)"
                  >
                    <div class="w-6 h-6 rounded-full bg-zinc-800 border border-zinc-700 flex items-center justify-center shrink-0">
                      <Users class="w-3 h-3 text-zinc-500" />
                    </div>
                    <span class="text-xs font-medium italic">Unsorted</span>
                    <span v-if="!shot.primary_person_id" class="ml-auto text-[10px] text-indigo-400">current</span>
                  </button>

                  <button
                    v-for="person in filteredReassignShotPeople"
                    :key="person.id"
                    class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-left transition-colors"
                    :class="shot.primary_person_id === person.id
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
                    <span v-if="shot.primary_person_id === person.id" class="ml-auto text-[10px] text-indigo-400">current</span>
                  </button>

                  <p v-if="filteredReassignShotPeople.length === 0" class="text-xs text-zinc-500 text-center py-3">No people found</p>
                </div>
              </ScrollArea>
            </div>
          </div>
        </div>
      </div>

      <!-- Split Mode Bar -->
      <div
        v-if="splitMode"
        class="flex items-center gap-3 p-3 rounded-xl bg-indigo-600/10 border border-indigo-500/20"
      >
        <Scissors class="w-4 h-4 text-indigo-400 shrink-0" />
        <p class="text-sm text-indigo-300 flex-1">
          Select files to split into a new shot.
          <span class="text-indigo-400 font-medium">{{ splitSelection.size }} selected</span>
        </p>
        <Button
          variant="ghost"
          size="sm"
          class="text-indigo-300 hover:text-white hover:bg-indigo-600/20"
          :disabled="splitSelection.size === 0 || splitSelection.size >= (shot.files?.length || 0)"
          @click="confirmSplit"
        >
          <Check class="w-3.5 h-3.5 mr-1" />
          Confirm Split
        </Button>
        <Button
          variant="ghost"
          size="sm"
          class="text-zinc-400 hover:text-white hover:bg-white/5"
          @click="exitSplitMode"
        >
          <X class="w-3.5 h-3.5 mr-1" />
          Cancel
        </Button>
      </div>

      <!-- Filmstrip (horizontal thumbnails) -->
      <div v-if="shot.files?.length > 1" class="relative">
        <div class="flex gap-2 overflow-x-auto pb-2 scrollbar-thin">
          <button
            v-for="(file, index) in shot.files"
            :key="file.id"
            class="relative shrink-0 w-20 h-20 rounded-lg overflow-hidden border-2 transition-all group/thumb"
            :class="[
              splitMode
                ? splitSelection.has(file.id)
                  ? 'border-indigo-500 ring-2 ring-indigo-500/30'
                  : 'border-white/10 hover:border-white/30'
                : selectedFileIndex === index
                  ? 'border-indigo-500 ring-2 ring-indigo-500/30'
                  : 'border-white/10 hover:border-white/30',
            ]"
            @click="splitMode ? toggleSplitFile(file.id) : (selectedFileIndex = index)"
          >
            <img
              :src="`/api/files/${file.id}/thumbnail`"
              class="w-full h-full object-cover"
              loading="lazy"
            />

            <!-- Original badge -->
            <div
              v-if="file.is_original"
              class="absolute top-1 left-1 w-5 h-5 rounded-full bg-yellow-500/90 flex items-center justify-center"
              title="Original"
            >
              <Star class="w-3 h-3 text-black" />
            </div>

            <!-- Video indicator -->
            <div
              v-if="(file.mime_type || '').startsWith('video/')"
              class="absolute bottom-1 right-1 w-5 h-5 rounded-full bg-black/60 flex items-center justify-center"
            >
              <Film class="w-3 h-3 text-white" />
            </div>

            <!-- Split selection check -->
            <div
              v-if="splitMode && splitSelection.has(file.id)"
              class="absolute inset-0 bg-indigo-600/20 flex items-center justify-center"
            >
              <div class="w-6 h-6 rounded-full bg-indigo-500 flex items-center justify-center">
                <Check class="w-4 h-4 text-white" />
              </div>
            </div>
          </button>
        </div>
      </div>

      <!-- Main content: Image + Metadata side by side on large screens -->
      <div class="flex flex-col lg:flex-row gap-6">
        <!-- Main image area -->
        <div class="flex-1 min-w-0">
          <!-- Selected file with face overlays -->
          <div class="relative rounded-xl overflow-visible bg-zinc-900 border border-white/5">
            <!-- Video in playback mode -->
            <video
              v-if="selectedFileUrl && isVideo && videoPlaying"
              :src="selectedFileUrl"
              controls
              autoplay
              class="w-full max-h-[60vh] object-contain bg-black rounded-xl"
            />

            <!-- Image (or video thumbnail with play button) with face overlays -->
            <div
              v-else-if="selectedFileUrl"
              class="relative"
            >
              <img
                :src="isVideo ? selectedFileThumbnailUrl : selectedFileUrl"
                :alt="selectedFilename"
                class="w-full max-h-[60vh] object-contain block rounded-xl"
                draggable="false"
                @load="onImageLoad"
              />

              <!-- Play button overlay for videos -->
              <button
                v-if="isVideo"
                class="absolute inset-0 flex items-center justify-center bg-black/20 hover:bg-black/30 transition-colors cursor-pointer group/play"
                @click="videoPlaying = true"
              >
                <div class="w-16 h-16 rounded-full bg-black/60 group-hover/play:bg-black/80 flex items-center justify-center backdrop-blur-sm transition-colors">
                  <Play class="w-8 h-8 text-white fill-white ml-1" />
                </div>
              </button>

              <!-- Face bounding boxes -->
              <template v-if="facesForSelectedFile.length">
                <div
                  v-for="face in facesForSelectedFile"
                  :key="face.id"
                  class="absolute border-2 border-indigo-400/70 rounded-sm cursor-pointer hover:border-indigo-300 transition-colors group/face"
                  :style="faceStyle(face)"
                  @click.stop="openReassign(face.id)"
                >
                  <!-- Person name label -->
                  <div class="absolute -bottom-6 left-1/2 -translate-x-1/2 whitespace-nowrap">
                    <span class="px-1.5 py-0.5 text-[10px] font-medium bg-black/70 text-zinc-200 rounded backdrop-blur-sm group-hover/face:bg-indigo-600/80 group-hover/face:text-white transition-colors">
                      {{ face.person_name || personName(face.person_id) || 'Unknown' }}
                    </span>
                  </div>

                  <!-- Face reassign popover -->
                  <div
                    v-if="reassignFaceId === face.id"
                    class="absolute top-full left-1/2 -translate-x-1/2 mt-8 z-50"
                    @click.stop
                  >
                    <div class="w-56 bg-zinc-900 border border-white/10 rounded-xl shadow-2xl overflow-hidden">
                      <div class="p-2 border-b border-white/5">
                        <div class="flex items-center justify-between mb-1.5 px-1">
                          <span class="text-xs font-semibold text-zinc-400">Reassign face</span>
                          <div class="flex items-center gap-1">
                            <!-- Delete face button -->
                            <button
                              class="p-1 rounded text-zinc-500 hover:text-red-400 hover:bg-red-500/10 transition-colors"
                              title="Delete face detection"
                              @click.stop="deleteFace(face.id)"
                            >
                              <Trash2 class="w-3.5 h-3.5" />
                            </button>
                            <button
                              class="text-zinc-500 hover:text-white transition-colors"
                              @click.stop="closeReassign"
                            >
                              <X class="w-3.5 h-3.5" />
                            </button>
                          </div>
                        </div>
                        <Input
                          v-model="reassignSearch"
                          placeholder="Search people..."
                          class="h-7 text-xs bg-zinc-800/50 border-white/5"
                          @click.stop
                        />
                      </div>
                      <ScrollArea class="max-h-48">
                        <div class="p-1">
                          <div v-if="reassigning" class="flex items-center justify-center py-4">
                            <RefreshCw class="w-4 h-4 text-indigo-400 animate-spin" />
                          </div>
                          <template v-else>
                            <button
                              v-for="person in filteredPeople"
                              :key="person.id"
                              class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-left transition-colors"
                              :class="face.person_id === person.id
                                ? 'bg-indigo-600/20 text-indigo-300'
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
                            </button>
                            <p v-if="filteredPeople.length === 0" class="text-xs text-zinc-500 text-center py-3">No people found</p>
                          </template>
                        </div>
                      </ScrollArea>
                    </div>
                  </div>
                </div>
              </template>
            </div>

            <!-- Loading placeholder -->
            <div v-else class="w-full h-64 flex items-center justify-center">
              <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
            </div>
          </div>

          <!-- File info bar below image -->
          <div class="mt-3 flex items-center justify-between gap-4">
            <div class="flex items-center gap-3 min-w-0 flex-wrap">
              <span class="text-sm font-medium text-zinc-300 truncate">{{ selectedFilename }}</span>
              <span v-if="selectedFile?.is_original" class="flex items-center gap-1 px-1.5 py-0.5 rounded bg-yellow-500/10 border border-yellow-500/20">
                <Star class="w-3 h-3 text-yellow-500" />
                <span class="text-[10px] font-medium text-yellow-400">Original</span>
              </span>
              <span v-if="shot.width && shot.height" class="text-xs text-zinc-500">
                {{ shot.width }} &times; {{ shot.height }}
              </span>
              <span v-if="formatFileSize(selectedFile?.file_size)" class="text-xs text-zinc-500">
                {{ formatFileSize(selectedFile.file_size) }}
              </span>
              <span v-if="facesForSelectedFile.length" class="flex items-center gap-1 text-xs text-zinc-500">
                <User class="w-3 h-3" />
                {{ facesForSelectedFile.length }} face{{ facesForSelectedFile.length !== 1 ? 's' : '' }}
              </span>
            </div>

            <div v-if="selectedFile && !selectedFile.is_original" class="flex items-center gap-2 shrink-0">
              <!-- Set as original button -->
              <Button
                variant="ghost"
                size="sm"
                class="gap-1.5 text-zinc-400 hover:text-yellow-400 hover:bg-yellow-500/10"
                @click="setOriginal(selectedFile.id)"
              >
                <Star class="w-3.5 h-3.5" />
                Set as Original
              </Button>
              <!-- Delete copy button -->
              <Button
                v-if="!confirmDeleteFile"
                variant="ghost"
                size="sm"
                class="gap-1.5 text-zinc-400 hover:text-red-400 hover:bg-red-500/10"
                @click="confirmDeleteFile = true"
              >
                <Trash2 class="w-3.5 h-3.5" />
                Delete Copy
              </Button>
              <!-- Confirm / Cancel -->
              <template v-else>
                <Button
                  variant="ghost"
                  size="sm"
                  class="gap-1.5 text-red-400 hover:text-red-300 hover:bg-red-500/10"
                  :disabled="deletingFile"
                  @click="deleteFileCopy(selectedFile.id)"
                >
                  <Trash2 v-if="!deletingFile" class="w-3.5 h-3.5" />
                  <RefreshCw v-else class="w-3.5 h-3.5 animate-spin" />
                  Confirm Delete
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  class="text-zinc-500 hover:text-zinc-300"
                  :disabled="deletingFile"
                  @click="confirmDeleteFile = false"
                >
                  Cancel
                </Button>
              </template>
            </div>
          </div>
        </div>

        <!-- Metadata Panel (right side on large screens, below on small) -->
        <div class="lg:w-72 shrink-0 space-y-4">
          <!-- Metadata card -->
          <div class="rounded-xl bg-zinc-800/30 border border-white/5 p-4 space-y-3">
            <h3 class="text-sm font-semibold text-zinc-300">Metadata</h3>

            <div v-if="metadata.length === 0" class="text-xs text-zinc-500 italic">
              No metadata available
            </div>

            <div v-for="item in metadata" :key="item.label" class="flex items-start gap-2.5">
              <div class="w-7 h-7 rounded-lg bg-zinc-800 flex items-center justify-center shrink-0 mt-0.5">
                <Clock v-if="item.icon === 'clock'" class="w-3.5 h-3.5 text-zinc-500" />
                <MapPin v-else-if="item.icon === 'map'" class="w-3.5 h-3.5 text-zinc-500" />
                <Maximize2 v-else-if="item.icon === 'size'" class="w-3.5 h-3.5 text-zinc-500" />
                <FileImage v-else-if="item.icon === 'files'" class="w-3.5 h-3.5 text-zinc-500" />
                <MessageSquare v-else-if="item.icon === 'caption'" class="w-3.5 h-3.5 text-zinc-500" />
                <HardDrive v-else class="w-3.5 h-3.5 text-zinc-500" />
              </div>
              <div class="min-w-0">
                <p class="text-[10px] text-zinc-500 font-medium uppercase tracking-wider">{{ item.label }}</p>
                <p class="text-xs text-zinc-300 break-all">{{ item.value }}</p>
              </div>
            </div>
          </div>

          <!-- Also contains -->
          <div
            v-if="shot.also_contains?.length"
            class="rounded-xl bg-zinc-800/30 border border-white/5 p-4 space-y-3"
          >
            <h3 class="text-sm font-semibold text-zinc-300">Also appears</h3>
            <div class="space-y-2">
              <router-link
                v-for="person in shot.also_contains"
                :key="person.id"
                :to="`/person/${person.id}`"
                class="flex items-center gap-2 px-2 py-1.5 rounded-lg hover:bg-white/5 transition-colors"
              >
                <div class="w-6 h-6 rounded-full bg-zinc-800 border border-zinc-700 flex items-center justify-center shrink-0">
                  <span class="text-[10px] font-bold text-zinc-500">{{ (person.name || '?')[0] }}</span>
                </div>
                <span class="text-xs font-medium text-zinc-300">{{ person.name || 'Unnamed' }}</span>
              </router-link>
            </div>
          </div>

          <!-- Files list (detailed) -->
          <div class="rounded-xl bg-zinc-800/30 border border-white/5 p-4 space-y-3">
            <h3 class="text-sm font-semibold text-zinc-300">Files</h3>
            <div class="space-y-2">
              <div
                v-for="(file, index) in shot.files"
                :key="file.id"
                class="flex items-center gap-2 px-2 py-1.5 rounded-lg transition-colors cursor-pointer"
                :class="selectedFileIndex === index
                  ? 'bg-indigo-600/10 border border-indigo-500/20'
                  : 'hover:bg-white/5 border border-transparent'"
                @click="selectedFileIndex = index"
              >
                <div class="w-8 h-8 rounded bg-zinc-800 overflow-hidden shrink-0">
                  <img
                    :src="`/api/files/${file.id}/thumbnail`"
                    class="w-full h-full object-cover"
                    loading="lazy"
                  />
                </div>
                <div class="min-w-0 flex-1">
                  <p class="text-xs font-medium text-zinc-300 truncate">{{ file.path.split('/').pop() }}</p>
                  <p class="text-[10px] text-zinc-500">{{ file.mime_type || 'unknown' }}</p>
                </div>
                <div class="flex items-center gap-1 shrink-0">
                  <div
                    v-if="file.is_original"
                    class="w-5 h-5 rounded-full bg-yellow-500/20 flex items-center justify-center"
                    title="Original"
                  >
                    <Star class="w-3 h-3 text-yellow-500" />
                  </div>
                  <button
                    v-else
                    class="px-1.5 py-0.5 rounded text-[10px] font-medium text-zinc-500 hover:text-yellow-400 hover:bg-yellow-500/10 transition-colors"
                    @click.stop="setOriginal(file.id)"
                  >
                    Set original
                  </button>
                </div>
              </div>
            </div>
          </div>

          <!-- AI Enhancements -->
          <div
            v-if="shotTasks.length"
            class="rounded-xl bg-zinc-800/30 border border-white/5 p-4 space-y-3"
          >
            <h3 class="text-sm font-semibold text-zinc-300 flex items-center gap-2">
              <Wand2 class="w-3.5 h-3.5 text-indigo-400" />
              AI Enhancements
            </h3>
            <div class="space-y-2">
              <div
                v-for="task in shotTasks"
                :key="task.id"
                class="flex items-center gap-2 px-2 py-2 rounded-lg bg-zinc-900/30 border border-white/5"
              >
                <div class="flex-1 min-w-0">
                  <p class="text-xs font-medium text-zinc-300 truncate">{{ task.workflow_name || 'Enhancement' }}</p>
                  <div class="flex items-center gap-1.5 mt-0.5">
                    <span
                      :class="cn(
                        'px-1.5 py-0.5 rounded text-[10px] font-medium border',
                        taskStatusBadgeClass(task.status)
                      )"
                    >
                      <RefreshCw v-if="task.status === 'running'" class="w-2.5 h-2.5 inline animate-spin mr-0.5" />
                      {{ task.status }}
                    </span>
                  </div>
                  <p v-if="task.error" class="text-[10px] text-red-400 mt-0.5 truncate">{{ task.error }}</p>
                </div>
                <button
                  v-if="task.status === 'failed'"
                  class="p-1 rounded text-zinc-500 hover:text-indigo-400 hover:bg-indigo-500/10 transition-colors shrink-0"
                  title="Retry"
                  @click="retryTask(task.id)"
                >
                  <RotateCcw class="w-3.5 h-3.5" />
                </button>
                <Check v-if="task.status === 'completed'" class="w-4 h-4 text-emerald-400 shrink-0" />
                <AlertCircle v-if="task.status === 'failed'" class="w-4 h-4 text-red-400 shrink-0" />
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Similar shots grouped by person (below the main content area) -->
      <div
        v-for="group in similarShots"
        :key="group.person_id || 'unknown'"
        class="rounded-xl bg-zinc-800/30 border border-white/5 p-4 space-y-3"
      >
        <h3 class="text-sm font-semibold text-zinc-300">{{ group.person_name || 'Unknown Person' }} ({{ group.shots.length }})</h3>
        <div class="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 gap-3">
          <div
            v-for="sim in group.shots"
            :key="sim.id"
            class="relative group/sim"
          >
            <router-link :to="`/shot/${sim.id}`">
              <ShotCard :shot="sim" />
            </router-link>
            <!-- Merge button overlay -->
            <button
              class="absolute inset-0 flex items-center justify-center bg-black/60 opacity-0 group-hover/sim:opacity-100 transition-opacity rounded-lg"
              @click.prevent.stop="openMergeConfirm(sim, group.person_id)"
            >
              <span class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium transition-colors">
                <Merge class="w-3.5 h-3.5" />
                Merge
              </span>
            </button>
          </div>
        </div>
      </div>
    </template>

    <!-- Delete Confirmation Dialog -->
    <Dialog v-model:open="showDeleteDialog">
      <DialogContent class="sm:max-w-[400px]">
        <DialogHeader>
          <DialogTitle>Delete Shot</DialogTitle>
          <DialogDescription>
            This will permanently delete this shot and all its files. This action cannot be undone.
          </DialogDescription>
        </DialogHeader>
        <div class="flex items-center justify-end gap-2 mt-4">
          <Button
            variant="ghost"
            class="text-zinc-400"
            @click="showDeleteDialog = false"
          >
            Cancel
          </Button>
          <Button
            class="bg-red-600 hover:bg-red-500 text-white"
            :disabled="deleting"
            @click="deleteShot"
          >
            <RefreshCw v-if="deleting" class="w-4 h-4 mr-1 animate-spin" />
            <Trash2 v-else class="w-4 h-4 mr-1" />
            {{ deleting ? 'Deleting...' : 'Delete Shot' }}
          </Button>
        </div>
      </DialogContent>
    </Dialog>

    <!-- Merge Confirmation Dialog -->
    <Dialog v-model:open="showMergeConfirm">
      <DialogContent class="sm:max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Merge Shot</DialogTitle>
          <DialogDescription>
            This will move all files from the selected shot into this shot. The other shot will be deleted.
          </DialogDescription>
        </DialogHeader>
        <div v-if="mergeTargetShot" class="mt-3 rounded-xl overflow-hidden border border-white/10 bg-zinc-900">
          <img
            :src="mergeTargetShot.thumbnail_url"
            class="w-full max-h-[300px] object-contain bg-black"
          />
        </div>
        <div class="flex items-center justify-end gap-2 mt-4">
          <Button
            variant="ghost"
            class="text-zinc-400"
            @click="showMergeConfirm = false"
          >
            Cancel
          </Button>
          <Button
            class="bg-indigo-600 hover:bg-indigo-500 text-white"
            :disabled="merging"
            @click="confirmMerge"
          >
            <RefreshCw v-if="merging" class="w-4 h-4 mr-1 animate-spin" />
            <Merge v-else class="w-4 h-4 mr-1" />
            {{ merging ? 'Merging...' : 'Merge' }}
          </Button>
        </div>
      </DialogContent>
    </Dialog>

    <!-- Enhance Dialog -->
    <EnhanceDialog
      v-model:open="showEnhanceDialog"
      :shot-id="shotId"
      :file-id="selectedFile?.id"
      @task-created="onTaskCreated"
    />
  </div>
</template>

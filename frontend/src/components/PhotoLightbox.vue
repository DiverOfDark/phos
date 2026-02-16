<script setup>
import { ref, watch, computed, onMounted, onUnmounted } from 'vue'
import { X, ChevronLeft, ChevronRight, User, Trash2, RefreshCw } from 'lucide-vue-next'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'

const props = defineProps({
  photos: { type: Array, default: () => [] },
})

const emit = defineEmits(['deleted'])

const open = defineModel('open', { type: Boolean, default: false })
const currentIndex = defineModel('index', { type: Number, default: 0 })

const currentPhoto = computed(() => props.photos[currentIndex.value] || null)

const detail = ref(null)
const loadingDetail = ref(false)

watch(
  () => [open.value, currentPhoto.value?.id],
  async ([isOpen, photoId]) => {
    if (!isOpen || !photoId) {
      detail.value = null
      return
    }
    loadingDetail.value = true
    try {
      const res = await fetch(`/api/photos/${photoId}`)
      if (res.ok) {
        detail.value = await res.json()
      }
    } catch (e) {
      console.warn('Failed to fetch photo detail', e)
    } finally {
      loadingDetail.value = false
    }
  },
  { immediate: true }
)

function prev() {
  if (currentIndex.value > 0) currentIndex.value--
}
function next() {
  if (currentIndex.value < props.photos.length - 1) currentIndex.value++
}

function onKeydown(e) {
  if (!open.value) return
  if (reassignFaceId.value) {
    if (e.key === 'Escape') closeReassign()
    return
  }
  if (e.key === 'ArrowLeft') prev()
  else if (e.key === 'ArrowRight') next()
  else if (e.key === 'Escape') open.value = false
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

// --- Zoom & Pan ---
const scale = ref(1)
const translateX = ref(0)
const translateY = ref(0)
const isDragging = ref(false)
const dragStart = ref({ x: 0, y: 0 })
const translateStart = ref({ x: 0, y: 0 })
const imageRef = ref(null)

const imageTransform = computed(() =>
  `scale(${scale.value}) translate(${translateX.value}px, ${translateY.value}px)`
)

const isZoomed = computed(() => scale.value > 1)

function onWheel(e) {
  e.preventDefault()
  const delta = e.deltaY > 0 ? -0.15 : 0.15
  const newScale = Math.min(10, Math.max(1, scale.value + delta * scale.value))
  if (newScale === 1) {
    // Reset pan when zooming all the way out
    translateX.value = 0
    translateY.value = 0
  }
  scale.value = newScale
}

function onPointerDown(e) {
  if (scale.value <= 1) return
  isDragging.value = true
  dragStart.value = { x: e.clientX, y: e.clientY }
  translateStart.value = { x: translateX.value, y: translateY.value }
  e.currentTarget.setPointerCapture(e.pointerId)
}

function onPointerMove(e) {
  if (!isDragging.value) return
  const dx = (e.clientX - dragStart.value.x) / scale.value
  const dy = (e.clientY - dragStart.value.y) / scale.value
  translateX.value = translateStart.value.x + dx
  translateY.value = translateStart.value.y + dy
}

function onPointerUp() {
  isDragging.value = false
}

function resetZoom() {
  scale.value = 1
  translateX.value = 0
  translateY.value = 0
}

// Reset zoom when changing photo or closing
watch([currentIndex, open], () => {
  resetZoom()
})

const confirmingDelete = ref(false)
const deleting = ref(false)

async function deletePhoto() {
  if (!confirmingDelete.value) {
    confirmingDelete.value = true
    return
  }
  const photo = currentPhoto.value
  if (!photo) return

  deleting.value = true
  try {
    const res = await fetch(`/api/photos/${photo.id}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    emit('deleted', photo.id)

    // Navigate to next photo or close if it was the last one
    if (props.photos.length <= 1) {
      open.value = false
    } else if (currentIndex.value >= props.photos.length - 1) {
      currentIndex.value = Math.max(0, currentIndex.value - 1)
    }
  } catch (e) {
    console.error('Failed to delete photo', e)
  } finally {
    deleting.value = false
    confirmingDelete.value = false
  }
}

function cancelDelete() {
  confirmingDelete.value = false
}

// Reset confirm state when navigating
watch(currentIndex, () => {
  confirmingDelete.value = false
})

const mainFile = computed(() => detail.value?.files?.[0] || null)

const fullMediaUrl = computed(() => {
  if (!mainFile.value) return null
  return `/api/files/${mainFile.value.id}`
})

const isVideo = computed(() => {
  const mime = mainFile.value?.mime_type || ''
  return mime.startsWith('video/')
})

const filename = computed(() => {
  if (!mainFile.value) return ''
  return mainFile.value.path.split('/').pop()
})

const timestamp = computed(() => {
  return currentPhoto.value?.timestamp || null
})

// --- Face overlays ---
const showFaces = ref(true)
const naturalWidth = ref(0)
const naturalHeight = ref(0)

function onImageLoad(e) {
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

// --- People list (for name labels & reassign) ---
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
    console.warn('Failed to fetch people', e)
  }
}

// Fetch people when lightbox opens with faces
watch(
  () => open.value,
  (isOpen) => {
    if (isOpen) fetchPeople()
  },
  { immediate: true }
)

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

// --- Face reassign ---
const reassignFaceId = ref(null)
const reassignSearch = ref('')
const reassigning = ref(false)

function openReassign(faceId) {
  reassignFaceId.value = faceId
  reassignSearch.value = ''
}

function closeReassign() {
  reassignFaceId.value = null
  reassignSearch.value = ''
}

const filteredPeople = computed(() => {
  const q = reassignSearch.value.toLowerCase().trim()
  let list = people.value
  if (q) {
    list = list.filter(p => (p.name || 'unnamed').toLowerCase().includes(q))
  }
  return list
})

async function reassignFace(faceId, targetPersonId) {
  reassigning.value = true
  try {
    const res = await fetch(`/api/faces/${faceId}/person`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ person_id: targetPersonId }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)

    // Update the face in detail locally
    if (detail.value?.faces) {
      const face = detail.value.faces.find(f => f.id === faceId)
      if (face) face.person_id = targetPersonId
    }

    closeReassign()
  } catch (e) {
    console.error('Failed to reassign face', e)
  } finally {
    reassigning.value = false
  }
}

// Close reassign when navigating or closing
watch([currentIndex, open], () => {
  closeReassign()
})
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="fixed inset-0 z-[100] flex items-center justify-center">
      <!-- Overlay -->
      <div class="absolute inset-0 bg-black/90 backdrop-blur-sm" @click="reassignFaceId ? closeReassign() : isZoomed ? resetZoom() : (open = false)" />

      <!-- Top-right buttons -->
      <div class="absolute top-4 right-4 z-10 flex items-center gap-2">
        <button
          v-if="detail?.faces?.length && !isVideo"
          class="p-2 rounded-full transition-colors"
          :class="showFaces ? 'bg-indigo-600 text-white' : 'bg-black/50 text-white/70 hover:text-white hover:bg-black/70'"
          :title="showFaces ? 'Hide faces' : 'Show faces'"
          @click="showFaces = !showFaces"
        >
          <User class="w-5 h-5" />
        </button>
        <button
          v-if="!confirmingDelete"
          class="p-2 rounded-full bg-black/50 text-white/70 hover:text-red-400 hover:bg-black/70 transition-colors"
          title="Delete photo"
          @click="deletePhoto"
        >
          <Trash2 class="w-5 h-5" />
        </button>
        <template v-else>
          <span class="text-sm text-red-400 mr-1">Delete?</span>
          <button
            class="p-2 rounded-full bg-red-600 text-white hover:bg-red-500 transition-colors"
            :disabled="deleting"
            @click="deletePhoto"
          >
            <Trash2 class="w-5 h-5" />
          </button>
          <button
            class="p-2 rounded-full bg-black/50 text-white/70 hover:text-white hover:bg-black/70 transition-colors"
            @click="cancelDelete"
          >
            <X class="w-5 h-5" />
          </button>
        </template>
        <button
          class="p-2 rounded-full bg-black/50 text-white/70 hover:text-white hover:bg-black/70 transition-colors"
          @click="open = false"
        >
          <X class="w-5 h-5" />
        </button>
      </div>

      <!-- Previous -->
      <button
        v-if="currentIndex > 0"
        class="absolute left-4 z-10 p-2 rounded-full bg-black/50 text-white/70 hover:text-white hover:bg-black/70 transition-colors"
        @click.stop="prev"
      >
        <ChevronLeft class="w-6 h-6" />
      </button>

      <!-- Next -->
      <button
        v-if="currentIndex < photos.length - 1"
        class="absolute right-4 z-10 p-2 rounded-full bg-black/50 text-white/70 hover:text-white hover:bg-black/70 transition-colors"
        @click.stop="next"
      >
        <ChevronRight class="w-6 h-6" />
      </button>

      <!-- Image area -->
      <div
        class="relative z-10 max-w-full max-h-full flex flex-col items-center px-16"
        @click.stop
        @wheel.prevent="onWheel"
      >
        <div
          class="overflow-hidden rounded-lg"
          :class="{ 'cursor-grab': isZoomed && !isDragging, 'cursor-grabbing': isDragging }"
          @pointerdown="onPointerDown"
          @pointermove="onPointerMove"
          @pointerup="onPointerUp"
          @pointercancel="onPointerUp"
          @dblclick="resetZoom"
        >
          <video
            v-if="fullMediaUrl && isVideo"
            :src="fullMediaUrl"
            controls
            autoplay
            class="max-w-full max-h-[calc(100vh-8rem)] object-contain select-none"
            :style="{ transform: imageTransform, transformOrigin: 'center center' }"
          />
          <div
            v-else-if="fullMediaUrl"
            class="relative inline-block"
            :style="{ transform: imageTransform, transformOrigin: 'center center' }"
          >
            <img
              ref="imageRef"
              :src="fullMediaUrl"
              :alt="filename"
              class="max-w-full max-h-[calc(100vh-8rem)] object-contain select-none block"
              draggable="false"
              @load="onImageLoad"
            />
            <!-- Face bounding boxes -->
            <template v-if="showFaces && detail?.faces?.length">
              <div
                v-for="face in detail.faces"
                :key="face.id"
                class="absolute border-2 border-indigo-400/70 rounded-sm cursor-pointer hover:border-indigo-300 transition-colors group/face"
                :style="faceStyle(face)"
                @click.stop="openReassign(face.id)"
              >
                <!-- Person name label (bottom of bounding box) -->
                <div class="absolute -bottom-6 left-1/2 -translate-x-1/2 whitespace-nowrap">
                  <span class="px-1.5 py-0.5 text-[10px] font-medium bg-black/70 text-zinc-200 rounded backdrop-blur-sm group-hover/face:bg-indigo-600/80 group-hover/face:text-white transition-colors">
                    {{ personName(face.person_id) || 'Unknown' }}
                  </span>
                </div>

                <!-- Reassign popover -->
                <div
                  v-if="reassignFaceId === face.id"
                  class="absolute top-full left-1/2 -translate-x-1/2 mt-8 z-50"
                  @click.stop
                >
                  <div class="w-56 bg-zinc-900 border border-white/10 rounded-xl shadow-2xl overflow-hidden">
                    <div class="p-2 border-b border-white/5">
                      <div class="flex items-center justify-between mb-1.5 px-1">
                        <span class="text-xs font-semibold text-zinc-400">Reassign to</span>
                        <button
                          class="text-zinc-500 hover:text-white transition-colors"
                          @click.stop="closeReassign"
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
          <div v-else class="w-64 h-64 flex items-center justify-center">
            <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
          </div>
        </div>

        <!-- Info bar -->
        <div class="mt-4 flex items-center gap-4 text-sm text-zinc-400">
          <span v-if="filename" class="font-medium text-zinc-300">{{ filename }}</span>
          <span v-if="timestamp">{{ new Date(timestamp).toLocaleString() }}</span>
          <span v-if="detail?.faces?.length" class="flex items-center gap-1">
            <User class="w-3.5 h-3.5" />
            {{ detail.faces.length }} {{ detail.faces.length === 1 ? 'face' : 'faces' }}
          </span>
          <span v-if="isZoomed" class="text-indigo-400">{{ Math.round(scale * 100) }}%</span>
          <span class="text-zinc-600">{{ currentIndex + 1 }} / {{ photos.length }}</span>
        </div>
      </div>
    </div>
  </Teleport>
</template>

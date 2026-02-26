<script setup>
import { ref, computed, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import ShotCard from '@/components/ShotCard.vue'
import {
  ArrowLeft,
  Edit3,
  Check,
  X,
  Merge,
  Search,
  Trash2,
  Users,
  Image as ImageIcon,
} from 'lucide-vue-next'

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
</script>

<template>
  <div>
    <!-- Loading -->
    <div v-if="loading" class="flex items-center justify-center py-20">
      <div class="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin"></div>
    </div>

    <!-- Error state -->
    <div v-else-if="error && !person" class="text-center py-20">
      <p class="text-zinc-500 text-sm">Could not load person data. Is the backend running?</p>
      <Button
        variant="outline"
        class="mt-4 border-white/10 text-zinc-300"
        @click="router.push('/people')"
      >
        <ArrowLeft class="w-4 h-4 mr-2" />
        Back to People
      </Button>
    </div>

    <div v-else>
      <!-- Header -->
      <div class="flex items-start gap-4 mb-8">
        <!-- Back button -->
        <Button
          variant="ghost"
          size="icon"
          class="text-zinc-400 hover:text-white rounded-xl hover:bg-white/5 shrink-0 mt-1"
          @click="router.back()"
        >
          <ArrowLeft class="w-5 h-5" />
        </Button>

        <!-- Face thumbnail -->
        <div class="w-20 h-20 rounded-full bg-zinc-800 border-2 border-zinc-700 overflow-hidden flex items-center justify-center shrink-0">
          <img
            v-if="person?.thumbnail_url"
            :src="person.thumbnail_url"
            class="w-full h-full object-cover"
          />
          <Users v-else class="w-8 h-8 text-zinc-600" />
        </div>

        <!-- Name + info -->
        <div class="flex-1 min-w-0">
          <!-- Editable name -->
          <div v-if="isEditingName" class="flex items-center gap-2 mb-1">
            <Input
              v-model="editName"
              class="h-9 text-lg font-bold bg-zinc-800/50 border-white/10"
              placeholder="Enter name..."
              @keydown.enter="saveName"
              @keydown.escape="cancelEditName"
              autofocus
            />
            <Button
              variant="ghost"
              size="icon"
              class="text-emerald-500 hover:text-emerald-400 hover:bg-emerald-500/10 shrink-0"
              :disabled="savingName || !editName.trim()"
              @click="saveName"
            >
              <Check class="w-4 h-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="text-zinc-400 hover:text-white hover:bg-white/5 shrink-0"
              @click="cancelEditName"
            >
              <X class="w-4 h-4" />
            </Button>
          </div>
          <div v-else class="flex items-center gap-2 mb-1">
            <h2 class="text-2xl font-bold text-white truncate">{{ displayName }}</h2>
            <Button
              variant="ghost"
              size="icon"
              class="text-zinc-500 hover:text-white hover:bg-white/5 shrink-0"
              @click="startEditName"
              title="Rename"
            >
              <Edit3 class="w-4 h-4" />
            </Button>
          </div>

          <div class="flex items-center gap-4 text-sm text-zinc-400">
            <span>{{ shots.length }} {{ shots.length === 1 ? 'shot' : 'shots' }}</span>
            <span v-if="person?.face_count">{{ person.face_count }} {{ person.face_count === 1 ? 'face' : 'faces' }}</span>
            <span v-if="person?.pending_count > 0" class="text-yellow-500">{{ person.pending_count }} pending</span>
          </div>

          <!-- Actions row -->
          <div class="flex items-center gap-2 mt-3">
            <Button
              variant="outline"
              size="sm"
              class="border-white/10 text-zinc-300 hover:text-white gap-1.5"
              @click="showMergeDialog = true"
            >
              <Merge class="w-3.5 h-3.5" />
              Merge with...
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="border-red-500/30 text-red-400 hover:text-red-300 hover:bg-red-500/10 gap-1.5"
              @click="showDeleteDialog = true"
            >
              <Trash2 class="w-3.5 h-3.5" />
              Delete
            </Button>
          </div>
        </div>
      </div>

      <!-- Shots grid -->
      <div v-if="shots.length > 0" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-3">
        <ShotCard
          v-for="shot in shots"
          :key="shot.id"
          :shot="shot"
          @click="navigateToShot(shot.id)"
        />
      </div>

      <!-- Empty shots state -->
      <div v-else class="text-center py-16">
        <ImageIcon class="w-12 h-12 text-zinc-700 mx-auto mb-4" />
        <p class="text-white font-medium mb-2">No shots found</p>
        <p class="text-zinc-500 text-sm">This person does not have any shots assigned yet.</p>
      </div>
    </div>

    <!-- Merge Dialog -->
    <Dialog v-model:open="showMergeDialog">
      <DialogContent class="sm:max-w-[420px] max-h-[70vh] overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle>Merge "{{ displayName }}" with...</DialogTitle>
          <DialogDescription>
            All faces from this person will be moved to the selected target person. This person will be deleted.
          </DialogDescription>
        </DialogHeader>

        <div class="flex flex-col gap-3 min-h-0 mt-2">
          <!-- Search filter -->
          <div class="relative">
            <Search class="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500" />
            <Input
              v-model="mergeFilter"
              placeholder="Search people..."
              class="pl-9 h-8 text-sm"
            />
          </div>

          <!-- People list -->
          <ScrollArea class="max-h-72 rounded-lg border border-white/5">
            <div class="p-1 space-y-0.5">
              <button
                v-for="target in filteredMergePeople"
                :key="target.id"
                @click="mergeWith(target.id)"
                :disabled="merging"
                class="w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left hover:bg-white/5 transition-colors group disabled:opacity-50"
              >
                <div class="w-10 h-10 rounded-full bg-zinc-800 border border-white/10 overflow-hidden flex items-center justify-center shrink-0">
                  <img
                    v-if="target.thumbnail_url"
                    :src="target.thumbnail_url"
                    class="w-full h-full object-cover"
                  />
                  <span v-else class="text-sm font-bold text-zinc-500">{{ (target.name || '?')[0] }}</span>
                </div>
                <div class="flex-1 min-w-0">
                  <span class="text-sm text-zinc-300 group-hover:text-white truncate block">
                    {{ target.name || 'Unnamed' }}
                  </span>
                  <span class="text-[10px] text-zinc-600">
                    {{ target.face_count }} faces / {{ target.shot_count || 0 }} shots
                  </span>
                </div>
              </button>
              <p v-if="filteredMergePeople.length === 0" class="text-xs text-zinc-500 text-center py-6">
                No other people to merge with
              </p>
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>

    <!-- Delete Confirmation Dialog -->
    <Dialog v-model:open="showDeleteDialog">
      <DialogContent class="sm:max-w-[400px]">
        <DialogHeader>
          <DialogTitle>Delete "{{ displayName }}"?</DialogTitle>
          <DialogDescription>
            This will remove all face markers from all photos of this person and delete the person record. The photos themselves will not be deleted.
          </DialogDescription>
        </DialogHeader>
        <div class="flex justify-end gap-2 mt-4">
          <Button
            variant="outline"
            class="border-white/10 text-zinc-300"
            :disabled="deleting"
            @click="showDeleteDialog = false"
          >
            Cancel
          </Button>
          <Button
            class="bg-red-600 hover:bg-red-700 text-white"
            :disabled="deleting"
            @click="deletePerson"
          >
            {{ deleting ? 'Deleting...' : 'Delete' }}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  </div>
</template>

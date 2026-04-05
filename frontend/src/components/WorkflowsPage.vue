<script setup>
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Wand2,
  Plus,
  Trash2,
  RefreshCw,
  Check,
  AlertCircle,
  X,
  ChevronRight,
  Clock,
  RotateCcw,
  ExternalLink,
  FileJson,
  ChevronDown,
  ChevronUp,
  Pencil,
  Save,
} from 'lucide-vue-next'

// --- Connection health ---
const comfyuiHealthy = ref(false)
const healthChecking = ref(true)

async function checkHealth() {
  healthChecking.value = true
  try {
    const res = await fetch('/api/comfyui/health')
    if (!res.ok) throw new Error()
    const data = await res.json()
    comfyuiHealthy.value = data.status === 'ok'
  } catch {
    comfyuiHealthy.value = false
  } finally {
    healthChecking.value = false
  }
}

// ===== WORKFLOWS TAB =====
const workflows = ref([])
const loadingWorkflows = ref(false)
const selectedWorkflowId = ref(null)

const selectedWorkflow = computed(() =>
  workflows.value.find(w => w.id === selectedWorkflowId.value) || null
)

async function fetchWorkflows() {
  loadingWorkflows.value = true
  try {
    const res = await fetch('/api/comfyui/workflows')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    workflows.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch workflows', e)
  } finally {
    loadingWorkflows.value = false
  }
}

async function deleteWorkflow(id) {
  if (!confirm('Delete this workflow?')) return
  try {
    const res = await fetch(`/api/comfyui/workflows/${id}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (selectedWorkflowId.value === id) selectedWorkflowId.value = null
    await fetchWorkflows()
  } catch (e) {
    console.error('Failed to delete workflow', e)
  }
}

// --- Import form ---
const showImportForm = ref(false)
const importName = ref('')
const importDescription = ref('')
const importJson = ref('')
const importing = ref(false)
const importError = ref('')
const importSuccess = ref(false)

function openImportForm() {
  showImportForm.value = true
  selectedWorkflowId.value = null
  importName.value = ''
  importDescription.value = ''
  importJson.value = ''
  importError.value = ''
  importSuccess.value = false
}

async function importWorkflow() {
  if (!importName.value.trim() || !importJson.value.trim()) return
  importing.value = true
  importError.value = ''
  importSuccess.value = false

  try {
    // Validate JSON
    JSON.parse(importJson.value)
  } catch {
    importError.value = 'Invalid JSON format'
    importing.value = false
    return
  }

  try {
    const res = await fetch('/api/comfyui/workflows', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: importName.value.trim(),
        description: importDescription.value.trim(),
        workflow: JSON.parse(importJson.value),
      }),
    })
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    importSuccess.value = true
    await fetchWorkflows()
    setTimeout(() => {
      showImportForm.value = false
      importSuccess.value = false
    }, 1000)
  } catch (e) {
    importError.value = e.message || 'Failed to import workflow'
  } finally {
    importing.value = false
  }
}

// --- JSON viewer toggle ---
const showRawJson = ref(false)

// ===== PRESETS =====
const presets = ref([])
const loadingPresets = ref(false)
const showAddPreset = ref(false)
const newPresetName = ref('')
const editingPresetId = ref(null)
const editingPresetName = ref('')

async function fetchPresets(workflowId) {
  if (!workflowId) {
    presets.value = []
    return
  }
  loadingPresets.value = true
  try {
    const res = await fetch(`/api/comfyui/workflows/${workflowId}/presets`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    presets.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch presets', e)
    presets.value = []
  } finally {
    loadingPresets.value = false
  }
}

async function createPreset() {
  if (!newPresetName.value.trim() || !selectedWorkflowId.value) return
  // Build text_overrides from workflow's current default inputs
  const overrides = {}
  const inputs = selectedWorkflow.value?.inputs || []
  for (const input of inputs) {
    if (input.node_type !== 'LoadImage') {
      overrides[`${input.node_id}.${input.field_name}`] = typeof input.current_value === 'string' ? input.current_value : ''
    }
  }
  try {
    const res = await fetch(`/api/comfyui/workflows/${selectedWorkflowId.value}/presets`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: newPresetName.value.trim(),
        text_overrides: overrides,
      }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    newPresetName.value = ''
    showAddPreset.value = false
    await fetchPresets(selectedWorkflowId.value)
  } catch (e) {
    console.error('Failed to create preset', e)
  }
}

function startEditPreset(preset) {
  editingPresetId.value = preset.id
  editingPresetName.value = preset.name
}

async function savePresetName(preset) {
  if (!editingPresetName.value.trim()) return
  try {
    const res = await fetch(`/api/comfyui/workflows/${selectedWorkflowId.value}/presets/${preset.id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: editingPresetName.value.trim(),
        text_overrides: preset.text_overrides,
        sort_order: preset.sort_order,
      }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    editingPresetId.value = null
    await fetchPresets(selectedWorkflowId.value)
  } catch (e) {
    console.error('Failed to update preset', e)
  }
}

async function updatePresetOverrides(preset, nodeId, value) {
  const updated = { ...preset.text_overrides, [nodeId]: value }
  try {
    const res = await fetch(`/api/comfyui/workflows/${selectedWorkflowId.value}/presets/${preset.id}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: preset.name,
        text_overrides: updated,
        sort_order: preset.sort_order,
      }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchPresets(selectedWorkflowId.value)
  } catch (e) {
    console.error('Failed to update preset overrides', e)
  }
}

async function deletePreset(presetId) {
  if (!confirm('Delete this preset?')) return
  try {
    const res = await fetch(`/api/comfyui/workflows/${selectedWorkflowId.value}/presets/${presetId}`, {
      method: 'DELETE',
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchPresets(selectedWorkflowId.value)
  } catch (e) {
    console.error('Failed to delete preset', e)
  }
}

// Fetch presets when selected workflow changes
watch(selectedWorkflowId, (id) => {
  if (id && !showImportForm.value) {
    fetchPresets(id)
  }
})

// ===== QUEUE TAB =====
const tasks = ref([])
const loadingTasks = ref(false)
const loadingMore = ref(false)
const nextCursor = ref(null)
let taskRefreshInterval = null

async function fetchTasks() {
  loadingTasks.value = true
  try {
    const res = await fetch('/api/comfyui/tasks?limit=50')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    tasks.value = data.items
    nextCursor.value = data.next_cursor
  } catch (e) {
    console.error('Failed to fetch tasks', e)
  } finally {
    loadingTasks.value = false
  }
}

async function fetchMoreTasks() {
  if (!nextCursor.value || loadingMore.value) return
  loadingMore.value = true
  try {
    const res = await fetch(`/api/comfyui/tasks?limit=50&cursor=${encodeURIComponent(nextCursor.value)}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    // Deduplicate by id in case polling shifted items
    const existingIds = new Set(tasks.value.map(t => t.id))
    const newItems = data.items.filter(t => !existingIds.has(t.id))
    tasks.value = [...tasks.value, ...newItems]
    nextCursor.value = data.next_cursor
  } catch (e) {
    console.error('Failed to fetch more tasks', e)
  } finally {
    loadingMore.value = false
  }
}

const taskListRef = ref(null)

function onTaskListScroll(event) {
  const el = event.target
  if (el.scrollHeight - el.scrollTop - el.clientHeight < 200) {
    fetchMoreTasks()
  }
}

const hasActiveTasks = computed(() =>
  tasks.value.some(t => t.status === 'pending' || t.status === 'running')
)

function startTaskPolling() {
  stopTaskPolling()
  taskRefreshInterval = setInterval(() => {
    fetchTasks()
  }, 5000)
}

function stopTaskPolling() {
  if (taskRefreshInterval) {
    clearInterval(taskRefreshInterval)
    taskRefreshInterval = null
  }
}

async function cancelTask(taskId) {
  try {
    const res = await fetch(`/api/comfyui/tasks/${taskId}/cancel`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchTasks()
  } catch (e) {
    console.error('Failed to cancel task', e)
  }
}

async function retryTask(taskId) {
  try {
    const res = await fetch(`/api/comfyui/tasks/${taskId}/retry`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchTasks()
  } catch (e) {
    console.error('Failed to retry task', e)
  }
}

async function deleteTask(taskId) {
  try {
    const res = await fetch(`/api/comfyui/tasks/${taskId}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchTasks()
  } catch (e) {
    console.error('Failed to delete task', e)
  }
}

function statusBadgeClass(status) {
  switch (status) {
    case 'completed':
      return 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20'
    case 'failed':
      return 'bg-red-500/10 text-red-400 border-red-500/20'
    case 'running':
      return 'bg-indigo-500/10 text-indigo-400 border-indigo-500/20'
    case 'cancelled':
      return 'bg-zinc-500/10 text-zinc-400 border-zinc-500/20'
    case 'pending':
    default:
      return 'bg-yellow-500/10 text-yellow-400 border-yellow-500/20'
  }
}

function formatRelativeTime(dateStr) {
  if (!dateStr) return ''
  const diff = Date.now() - new Date(dateStr).getTime()
  const secs = Math.floor(diff / 1000)
  if (secs < 60) return 'just now'
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  return `${days}d ago`
}

function formatDate(dateStr) {
  if (!dateStr) return ''
  return new Date(dateStr).toLocaleDateString()
}

// --- Active tab tracking (synced with URL) ---
const route = useRoute()
const router = useRouter()
const activeTab = computed(() => route.query.tab === 'queue' ? 'queue' : 'workflows')

function onTabChange(val) {
  router.replace({ query: { ...route.query, tab: val === 'workflows' ? undefined : val } })
  if (val === 'queue') {
    fetchTasks()
    startTaskPolling()
  } else {
    stopTaskPolling()
  }
}

// --- Lifecycle ---
onMounted(() => {
  checkHealth()
  fetchWorkflows()
  if (activeTab.value === 'queue') {
    fetchTasks()
    startTaskPolling()
  }
})

onUnmounted(() => {
  stopTaskPolling()
})
</script>

<template>
  <div class="space-y-6">
    <!-- Header -->
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <div class="w-10 h-10 rounded-xl bg-indigo-600/10 border border-indigo-500/20 flex items-center justify-center">
          <Wand2 class="w-5 h-5 text-indigo-400" />
        </div>
        <div>
          <h2 class="text-xl font-bold text-white">Workflows</h2>
          <p class="text-zinc-500 text-xs mt-0.5">ComfyUI workflow management</p>
        </div>
      </div>

      <!-- Connection status -->
      <div class="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-zinc-800/50 border border-white/5">
        <div
          :class="cn(
            'w-2 h-2 rounded-full',
            healthChecking ? 'bg-zinc-500 animate-pulse' : comfyuiHealthy ? 'bg-emerald-500' : 'bg-red-500'
          )"
        />
        <span class="text-xs font-medium text-zinc-400">
          {{ healthChecking ? 'Checking...' : comfyuiHealthy ? 'Connected' : 'Disconnected' }}
        </span>
      </div>
    </div>

    <!-- Tabs -->
    <Tabs :model-value="activeTab" @update:model-value="onTabChange">
      <TabsList class="grid w-full grid-cols-2 max-w-xs">
        <TabsTrigger value="workflows">Workflows</TabsTrigger>
        <TabsTrigger value="queue">Queue</TabsTrigger>
      </TabsList>

      <!-- ===== WORKFLOWS TAB ===== -->
      <TabsContent value="workflows" class="mt-6">
        <div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
          <!-- Left column: Workflow list -->
          <div class="lg:col-span-1 space-y-3">
            <div class="flex items-center justify-between">
              <h3 class="text-sm font-semibold text-zinc-300">Available Workflows</h3>
              <Button
                variant="ghost"
                size="sm"
                class="gap-1.5 text-indigo-400 hover:text-indigo-300 hover:bg-indigo-500/10"
                @click="openImportForm"
              >
                <Plus class="w-3.5 h-3.5" />
                Import
              </Button>
            </div>

            <!-- Loading -->
            <div v-if="loadingWorkflows" class="flex items-center justify-center py-8">
              <RefreshCw class="w-5 h-5 text-indigo-400 animate-spin" />
            </div>

            <!-- Workflow cards -->
            <div v-else-if="workflows.length" class="space-y-2">
              <button
                v-for="wf in workflows"
                :key="wf.id"
                class="w-full text-left p-3 rounded-xl border transition-all group/wf"
                :class="selectedWorkflowId === wf.id && !showImportForm
                  ? 'bg-indigo-600/10 border-indigo-500/20'
                  : 'bg-zinc-800/30 border-white/5 hover:border-white/10 hover:bg-zinc-800/50'"
                @click="showImportForm = false; selectedWorkflowId = wf.id"
              >
                <div class="flex items-start justify-between gap-2">
                  <div class="min-w-0 flex-1">
                    <p class="text-sm font-medium text-zinc-200 truncate">{{ wf.name }}</p>
                    <p v-if="wf.description" class="text-xs text-zinc-500 mt-0.5 line-clamp-2">{{ wf.description }}</p>
                  </div>
                  <button
                    class="p-1 rounded text-zinc-600 hover:text-red-400 hover:bg-red-500/10 transition-colors opacity-0 group-hover/wf:opacity-100 shrink-0"
                    @click.stop="deleteWorkflow(wf.id)"
                  >
                    <Trash2 class="w-3.5 h-3.5" />
                  </button>
                </div>

                <div class="flex items-center gap-2 mt-2">
                  <span
                    v-if="wf.outputs?.length"
                    class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-indigo-500/10 text-indigo-400 border border-indigo-500/20"
                  >
                    {{ wf.outputs[0].node_type || 'output' }}
                  </span>
                  <span v-if="wf.inputs?.length" class="text-[10px] text-zinc-500">
                    {{ wf.inputs.length }} input{{ wf.inputs.length !== 1 ? 's' : '' }}
                  </span>
                  <span class="text-[10px] text-zinc-600 ml-auto">{{ formatDate(wf.created_at) }}</span>
                </div>
              </button>
            </div>

            <!-- Empty state -->
            <div v-else class="text-center py-8">
              <Wand2 class="w-8 h-8 text-zinc-700 mx-auto mb-2" />
              <p class="text-sm text-zinc-500">No workflows yet</p>
              <p class="text-xs text-zinc-600 mt-1">Import a ComfyUI workflow to get started.</p>
            </div>
          </div>

          <!-- Right column: Detail or Import panel -->
          <div class="lg:col-span-2">
            <!-- Import form -->
            <div v-if="showImportForm" class="rounded-xl bg-zinc-800/30 border border-white/5 p-6 space-y-4">
              <div class="flex items-center justify-between">
                <h3 class="text-sm font-semibold text-zinc-200">Import Workflow</h3>
                <button class="text-zinc-500 hover:text-white transition-colors" @click="showImportForm = false">
                  <X class="w-4 h-4" />
                </button>
              </div>

              <div class="space-y-2">
                <Label>Name</Label>
                <Input v-model="importName" placeholder="My Upscale Workflow" class="bg-zinc-900/50 border-white/10" />
              </div>

              <div class="space-y-2">
                <Label>Description</Label>
                <textarea
                  v-model="importDescription"
                  rows="2"
                  placeholder="Optional description..."
                  class="flex w-full rounded-lg border border-white/10 bg-zinc-900/50 px-3 py-2 text-sm text-zinc-200 placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500/40 focus-visible:ring-offset-0 resize-y"
                />
              </div>

              <div class="space-y-2">
                <Label>Workflow JSON</Label>
                <textarea
                  v-model="importJson"
                  rows="15"
                  placeholder='Paste your ComfyUI workflow JSON here...'
                  class="flex w-full rounded-lg border border-white/10 bg-zinc-900/50 px-3 py-2 text-sm text-zinc-200 placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500/40 focus-visible:ring-offset-0 resize-y font-mono text-xs"
                />
              </div>

              <!-- Feedback -->
              <div v-if="importError" class="flex items-start gap-2 p-3 rounded-xl bg-red-500/10 border border-red-500/20">
                <AlertCircle class="w-4 h-4 text-red-500 mt-0.5 shrink-0" />
                <p class="text-sm text-red-400">{{ importError }}</p>
              </div>
              <div v-if="importSuccess" class="flex items-start gap-2 p-3 rounded-xl bg-emerald-500/10 border border-emerald-500/20">
                <Check class="w-4 h-4 text-emerald-500 mt-0.5 shrink-0" />
                <p class="text-sm text-emerald-400">Workflow imported successfully!</p>
              </div>

              <Button
                class="w-full bg-indigo-600 hover:bg-indigo-500 text-white gap-2"
                :disabled="!importName.trim() || !importJson.trim() || importing"
                @click="importWorkflow"
              >
                <RefreshCw v-if="importing" class="w-4 h-4 animate-spin" />
                <Plus v-else class="w-4 h-4" />
                {{ importing ? 'Importing...' : 'Import Workflow' }}
              </Button>
            </div>

            <!-- Workflow detail -->
            <div v-else-if="selectedWorkflow" class="rounded-xl bg-zinc-800/30 border border-white/5 p-6 space-y-5">
              <div class="flex items-start justify-between gap-4">
                <div>
                  <h3 class="text-lg font-semibold text-white">{{ selectedWorkflow.name }}</h3>
                  <p v-if="selectedWorkflow.description" class="text-sm text-zinc-400 mt-1">{{ selectedWorkflow.description }}</p>
                  <p class="text-xs text-zinc-500 mt-2">Created {{ formatDate(selectedWorkflow.created_at) }}</p>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  class="gap-1.5 text-zinc-500 hover:text-red-400 hover:bg-red-500/10 shrink-0"
                  @click="deleteWorkflow(selectedWorkflow.id)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                  Delete
                </Button>
              </div>

              <!-- Detected Inputs -->
              <div v-if="selectedWorkflow.inputs?.length" class="space-y-2">
                <h4 class="text-sm font-medium text-zinc-300">Detected Inputs</h4>
                <div class="space-y-1.5">
                  <div
                    v-for="input in selectedWorkflow.inputs"
                    :key="input.node_id"
                    class="flex items-center gap-2 px-3 py-2 rounded-lg bg-zinc-900/50 border border-white/5"
                  >
                    <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-zinc-800 text-zinc-400 border border-white/5">
                      {{ input.node_type }}
                    </span>
                    <span class="text-sm text-zinc-300">{{ input.field_name }} <span class="text-zinc-600">(node {{ input.node_id }})</span></span>
                    <span v-if="input.current_value && typeof input.current_value === 'string'" class="text-xs text-zinc-500 ml-auto truncate max-w-[200px]">
                      {{ input.current_value }}
                    </span>
                  </div>
                </div>
              </div>

              <!-- Detected Outputs -->
              <div v-if="selectedWorkflow.outputs?.length" class="space-y-2">
                <h4 class="text-sm font-medium text-zinc-300">Detected Outputs</h4>
                <div class="flex flex-wrap gap-2">
                  <span
                    v-for="(output, i) in selectedWorkflow.outputs"
                    :key="i"
                    class="px-2 py-1 rounded-lg text-xs font-medium bg-indigo-500/10 text-indigo-400 border border-indigo-500/20"
                  >
                    {{ output.node_type || 'output' }}
                  </span>
                </div>
              </div>

              <!-- Presets -->
              <div class="space-y-3">
                <div class="flex items-center justify-between">
                  <h4 class="text-sm font-medium text-zinc-300">Prompt Presets</h4>
                  <Button
                    v-if="!showAddPreset"
                    variant="ghost"
                    size="sm"
                    class="gap-1.5 text-indigo-400 hover:text-indigo-300 hover:bg-indigo-500/10"
                    @click="showAddPreset = true; newPresetName = ''"
                  >
                    <Plus class="w-3.5 h-3.5" />
                    Add
                  </Button>
                </div>

                <!-- Add preset form -->
                <div v-if="showAddPreset" class="flex items-center gap-2">
                  <Input
                    v-model="newPresetName"
                    placeholder="Preset name..."
                    class="bg-zinc-900/50 border-white/10 flex-1"
                    @keyup.enter="createPreset"
                  />
                  <Button
                    size="sm"
                    class="bg-indigo-600 hover:bg-indigo-500 text-white gap-1"
                    :disabled="!newPresetName.trim()"
                    @click="createPreset"
                  >
                    <Check class="w-3.5 h-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    class="text-zinc-500 hover:text-white"
                    @click="showAddPreset = false"
                  >
                    <X class="w-3.5 h-3.5" />
                  </Button>
                </div>

                <!-- Preset list -->
                <div v-if="loadingPresets" class="flex items-center justify-center py-4">
                  <RefreshCw class="w-4 h-4 text-indigo-400 animate-spin" />
                </div>
                <div v-else-if="presets.length" class="space-y-2">
                  <div
                    v-for="preset in presets"
                    :key="preset.id"
                    class="rounded-lg bg-zinc-900/50 border border-white/5 p-3 space-y-2"
                  >
                    <!-- Preset header -->
                    <div class="flex items-center justify-between gap-2">
                      <div v-if="editingPresetId === preset.id" class="flex items-center gap-2 flex-1">
                        <Input
                          v-model="editingPresetName"
                          class="bg-zinc-800/50 border-white/10 text-sm h-7 flex-1"
                          @keyup.enter="savePresetName(preset)"
                        />
                        <button class="text-emerald-400 hover:text-emerald-300" @click="savePresetName(preset)">
                          <Save class="w-3.5 h-3.5" />
                        </button>
                        <button class="text-zinc-500 hover:text-white" @click="editingPresetId = null">
                          <X class="w-3.5 h-3.5" />
                        </button>
                      </div>
                      <span v-else class="text-sm font-medium text-amber-300">{{ preset.name }}</span>
                      <div v-if="editingPresetId !== preset.id" class="flex items-center gap-1 shrink-0">
                        <button
                          class="p-1 rounded text-zinc-600 hover:text-zinc-300 hover:bg-white/5 transition-colors"
                          @click="startEditPreset(preset)"
                        >
                          <Pencil class="w-3 h-3" />
                        </button>
                        <button
                          class="p-1 rounded text-zinc-600 hover:text-red-400 hover:bg-red-500/10 transition-colors"
                          @click="deletePreset(preset.id)"
                        >
                          <Trash2 class="w-3 h-3" />
                        </button>
                      </div>
                    </div>

                    <!-- Preset text overrides (editable) -->
                    <div
                      v-for="input in (selectedWorkflow.inputs || []).filter(i => i.node_type !== 'LoadImage')"
                      :key="`${input.node_id}.${input.field_name}`"
                      class="space-y-1"
                    >
                      <label class="text-[10px] font-medium text-zinc-500">
                        {{ input.field_name }} ({{ input.node_type }})
                      </label>
                      <textarea
                        :value="preset.text_overrides[`${input.node_id}.${input.field_name}`] || ''"
                        rows="2"
                        class="flex w-full rounded-md border border-white/5 bg-zinc-800/30 px-2 py-1.5 text-xs text-zinc-300 placeholder:text-zinc-600 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-indigo-500/40 resize-y"
                        :placeholder="typeof input.current_value === 'string' ? input.current_value : ''"
                        @change="updatePresetOverrides(preset, `${input.node_id}.${input.field_name}`, $event.target.value)"
                      />
                    </div>
                  </div>
                </div>
                <p v-else class="text-xs text-zinc-600">No presets yet. Add one to save prompt text for quick reuse.</p>
              </div>

              <!-- Raw JSON viewer -->
              <div class="space-y-2">
                <button
                  class="flex items-center gap-1.5 text-xs font-medium text-zinc-500 hover:text-zinc-300 transition-colors"
                  @click="showRawJson = !showRawJson"
                >
                  <FileJson class="w-3.5 h-3.5" />
                  Raw JSON
                  <ChevronDown v-if="!showRawJson" class="w-3 h-3" />
                  <ChevronUp v-else class="w-3 h-3" />
                </button>
                <div v-if="showRawJson" class="max-h-64 overflow-auto rounded-lg bg-zinc-900 border border-white/5 p-3">
                  <pre class="text-xs text-zinc-400 font-mono whitespace-pre-wrap break-all">{{ JSON.stringify(selectedWorkflow, null, 2) }}</pre>
                </div>
              </div>
            </div>

            <!-- No selection -->
            <div v-else class="rounded-xl bg-zinc-800/30 border border-white/5 p-12 text-center">
              <ChevronRight class="w-8 h-8 text-zinc-700 mx-auto mb-3" />
              <p class="text-sm text-zinc-500">Select a workflow or import a new one</p>
            </div>
          </div>
        </div>
      </TabsContent>

      <!-- ===== QUEUE TAB ===== -->
      <TabsContent value="queue" class="mt-6">
        <div class="space-y-4">
          <!-- Queue header -->
          <div class="flex items-center justify-between">
            <h3 class="text-sm font-semibold text-zinc-300">Task Queue</h3>
            <Button
              variant="ghost"
              size="sm"
              class="gap-1.5 text-zinc-400 hover:text-white hover:bg-white/5"
              @click="fetchTasks"
            >
              <RefreshCw :class="cn('w-3.5 h-3.5', loadingTasks && 'animate-spin')" />
              Refresh
            </Button>
          </div>

          <!-- Loading -->
          <div v-if="loadingTasks && !tasks.length" class="flex items-center justify-center py-12">
            <RefreshCw class="w-5 h-5 text-indigo-400 animate-spin" />
          </div>

          <!-- Task list -->
          <div v-else-if="tasks.length" ref="taskListRef" class="space-y-2 max-h-[calc(100vh-16rem)] overflow-y-auto" @scroll="onTaskListScroll">
            <div
              v-for="task in tasks"
              :key="task.id"
              class="flex items-center gap-4 p-4 rounded-xl bg-zinc-800/30 border border-white/5 hover:border-white/10 transition-colors"
            >
              <!-- Shot thumbnail -->
              <router-link
                v-if="task.shot_id"
                :to="`/shot/${task.shot_id}`"
                class="w-12 h-12 rounded-lg overflow-hidden bg-zinc-800 border border-white/5 shrink-0 hover:border-indigo-500/30 transition-colors"
              >
                <img
                  v-if="task.thumbnail_url"
                  :src="task.thumbnail_url"
                  class="w-full h-full object-cover"
                  loading="lazy"
                />
                <div v-else class="w-full h-full flex items-center justify-center">
                  <Wand2 class="w-4 h-4 text-zinc-600" />
                </div>
              </router-link>
              <div v-else class="w-12 h-12 rounded-lg bg-zinc-800 border border-white/5 shrink-0 flex items-center justify-center">
                <Wand2 class="w-4 h-4 text-zinc-600" />
              </div>

              <!-- Task info -->
              <div class="flex-1 min-w-0">
                <p class="text-sm font-medium text-zinc-200 truncate">
                  {{ task.workflow_name || 'Unknown Workflow' }}
                </p>
                <div class="flex items-center gap-2 mt-1">
                  <span
                    :class="cn(
                      'px-1.5 py-0.5 rounded text-[10px] font-medium border',
                      statusBadgeClass(task.status)
                    )"
                  >
                    <RefreshCw v-if="task.status === 'running'" class="w-2.5 h-2.5 inline animate-spin mr-0.5" />
                    {{ task.status }}
                  </span>
                  <span v-if="task.retry_count > 0" class="text-[10px] text-zinc-500">
                    {{ task.retry_count }} {{ task.retry_count === 1 ? 'retry' : 'retries' }}
                  </span>
                  <span class="text-[10px] text-zinc-600">
                    {{ formatRelativeTime(task.created_at) }}
                  </span>
                </div>
                <p v-if="task.error" class="text-xs text-red-400 mt-1 truncate">{{ task.error }}</p>
              </div>

              <!-- Actions -->
              <div class="flex items-center gap-1 shrink-0">
                <Button
                  v-if="task.status === 'pending' || task.status === 'running'"
                  variant="ghost"
                  size="sm"
                  class="text-zinc-500 hover:text-red-400 hover:bg-red-500/10"
                  @click="cancelTask(task.id)"
                >
                  <X class="w-3.5 h-3.5" />
                </Button>
                <Button
                  v-if="task.status === 'failed'"
                  variant="ghost"
                  size="sm"
                  class="gap-1 text-zinc-500 hover:text-indigo-400 hover:bg-indigo-500/10"
                  @click="retryTask(task.id)"
                >
                  <RotateCcw class="w-3.5 h-3.5" />
                </Button>
                <Button
                  v-if="task.status === 'failed'"
                  variant="ghost"
                  size="sm"
                  class="gap-1 text-zinc-500 hover:text-red-400 hover:bg-red-500/10"
                  title="Remove"
                  @click="deleteTask(task.id)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
                <router-link
                  v-if="task.status === 'completed' && task.shot_id"
                  :to="`/shot/${task.shot_id}`"
                >
                  <Button
                    variant="ghost"
                    size="sm"
                    class="gap-1 text-zinc-500 hover:text-emerald-400 hover:bg-emerald-500/10"
                  >
                    <ExternalLink class="w-3.5 h-3.5" />
                  </Button>
                </router-link>
              </div>
            </div>
            <!-- Load more indicator -->
            <div v-if="loadingMore" class="flex items-center justify-center py-4">
              <RefreshCw class="w-4 h-4 text-indigo-400 animate-spin" />
            </div>
            <div v-else-if="nextCursor" class="flex items-center justify-center py-2">
              <span class="text-xs text-zinc-600">Scroll for more</span>
            </div>
          </div>

          <!-- Empty state -->
          <div v-else class="text-center py-12">
            <Clock class="w-8 h-8 text-zinc-700 mx-auto mb-2" />
            <p class="text-sm text-zinc-500">No tasks in the queue</p>
            <p class="text-xs text-zinc-600 mt-1">Enhance a shot to see tasks appear here.</p>
          </div>
        </div>
      </TabsContent>
    </Tabs>
  </div>
</template>

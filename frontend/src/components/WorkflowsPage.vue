<script setup>
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import WorkflowGraph from '@/components/WorkflowGraph.vue'
import WorkflowInputControls from '@/components/WorkflowInputControls.vue'
import { controlKind, isParameterInput, isTextInput, inputKey, parameterValue } from '@/lib/utils'

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
    // Land on something: an empty detail pane says nothing about the workflows.
    if (!selectedWorkflowId.value && workflows.value.length) {
      selectedWorkflowId.value = workflows.value[0].id
      fetchPresets(selectedWorkflowId.value)
    }
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
  // A new preset starts where the workflow does: its author's own prompts and
  // its author's own seed, steps and model, so saving one changes nothing until
  // it is edited. The fields the source picker fills are left out — a preset
  // that pinned a loader's filename would override the uploaded source at run
  // time, since typed parameters are applied after the binding.
  const loaders = selectedWorkflow.value?.loaders || []
  const isLoaderField = (input) =>
    input.node_type === 'LoadImage' ||
    loaders.some(l => l.node_id === input.node_id && l.field === input.field_name)
  const overrides = {}
  const parameters = {}
  for (const input of selectedWorkflow.value?.inputs || []) {
    if (isLoaderField(input)) continue
    if (isTextInput(input)) overrides[inputKey(input)] = String(input.current_value ?? '')
    else if (isParameterInput(input)) parameters[inputKey(input)] = parameterValue(input)
  }
  try {
    const res = await fetch(`/api/comfyui/workflows/${selectedWorkflowId.value}/presets`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: newPresetName.value.trim(),
        text_overrides: overrides,
        parameters,
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
        parameters: preset.parameters || {},
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

/** Pending saves, one timer per preset — a control is edited, not submitted. */
const presetSaveTimers = new Map()

/** Save a preset's values a moment after the last edit to it. */
function savePresetValues(preset) {
  // Captured now, not when the timer fires: selecting another workflow inside
  // the debounce window would otherwise send this preset to that workflow's
  // URL, where the backend answers 404 and the edit is silently lost.
  const workflowId = selectedWorkflowId.value
  clearTimeout(presetSaveTimers.get(preset.id))
  presetSaveTimers.set(
    preset.id,
    setTimeout(async () => {
      presetSaveTimers.delete(preset.id)
      try {
        const res = await fetch(
          `/api/comfyui/workflows/${workflowId}/presets/${preset.id}`,
          {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              name: preset.name,
              text_overrides: preset.text_overrides || {},
              parameters: preset.parameters || {},
              sort_order: preset.sort_order,
            }),
          },
        )
        if (!res.ok) throw new Error(`HTTP ${res.status}`)
      } catch (e) {
        console.error('Failed to update preset', e)
      }
    }, 600),
  )
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

const hasActiveTasks = computed(() => tasks.value.some(t => isInFlight(t.status)))

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

/**
 * The statuses the backend actually writes: pending → uploading → queued →
 * processing → downloading → completed, with awaiting_output when ComfyUI says
 * the run finished but has not published the file yet, and failed / cancelled
 * as the two ways it ends early.
 */
function statusColor(status) {
  switch (status) {
    case 'completed': return 'var(--status-ready)'
    case 'failed': return 'var(--status-error)'
    case 'cancelled': return 'var(--status-stopped)'
    case 'uploading':
    case 'queued':
    case 'processing':
    case 'downloading': return 'var(--status-building)'
    // Done executing, still waiting on the file. Not an error, not finished.
    case 'awaiting_output': return 'var(--status-degraded)'
    default: return 'var(--status-pending)'
  }
}

/** Statuses the worker is still moving through. */
const IN_FLIGHT = ['pending', 'uploading', 'queued', 'processing', 'downloading', 'awaiting_output']

function isInFlight(status) {
  return IN_FLIGHT.includes(status)
}

/** `awaiting_output` reads better as two words in the schedule register. */
function statusLabel(status) {
  return String(status || '').replace(/_/g, ' ')
}

/** Task ids are long; the queue shows the first 7, like a commit. */
function shortId(id) {
  return String(id || '').slice(0, 7)
}

/** Inputs a person can set — LoadImage is fed by the shot, never by hand. */
function editableInputsOf(wf) {
  return (wf?.inputs || []).filter((i) => controlKind(i) !== null)
}

const importReady = computed(() => importName.value.trim() && importJson.value.trim())

/** Node ids the Enhance dialog can override — the graph marks them as editable. */
const editableNodeIds = computed(() =>
  editableInputsOf(selectedWorkflow.value).map((i) => String(i.node_id))
)

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

defineExpose({ loadData: fetchWorkflows })
</script>

<template>
  <div class="p-4 md:p-8 max-w-[1040px] w-full mx-auto flex flex-col gap-6">
    <div class="flex flex-wrap items-baseline justify-between gap-4">
      <h2 class="text-[22px] font-semibold">Workflows</h2>
      <div class="flex items-center gap-2 font-mono text-xs text-ink-tertiary">
        <span
          class="signal-dot"
          style="width:6px;height:6px"
          :style="{ background: healthChecking ? 'var(--status-pending)' : comfyuiHealthy ? 'var(--status-ready)' : 'var(--status-error)' }"
        ></span>
        comfyui · {{ healthChecking ? 'checking' : comfyuiHealthy ? 'reachable' : 'unreachable' }}
      </div>
    </div>

    <div class="text-[13px] font-light text-ink-secondary max-w-[560px]">
      Imported ComfyUI workflows run on demand — open a shot and use
      <span class="font-normal text-ink">Enhance</span>. Results attach to the shot as a new file.
    </div>

    <!-- Tabs -->
    <div class="flex items-center gap-1 border-b border-line">
      <button
        v-for="t in [
          { id: 'workflows', label: 'Workflows', count: workflows.length },
          { id: 'queue', label: 'Queue', count: tasks.length },
        ]"
        :key="t.id"
        class="flex items-center gap-2 px-3 py-2 border-b-2 text-[13px] transition-colors"
        :class="activeTab === t.id
          ? 'border-signal text-ink font-medium'
          : 'border-transparent text-ink-secondary hover:text-ink'"
        @click="onTabChange(t.id)"
      >
        {{ t.label }}
        <span class="font-mono text-[11px] text-ink-tertiary">{{ t.count }}</span>
      </button>
    </div>

    <!-- Workflows tab -->
    <div v-if="activeTab === 'workflows'" class="grid gap-6 items-start lg:grid-cols-[320px_minmax(0,1fr)]">
      <div class="flex flex-col gap-2">
        <button
          v-for="wf in workflows"
          :key="wf.id"
          class="flex flex-col gap-1 p-4 border rounded text-left transition-colors"
          :class="selectedWorkflowId === wf.id && !showImportForm
            ? 'border-signal bg-surface'
            : 'border-line hover:bg-raised'"
          @click="showImportForm = false; selectedWorkflowId = wf.id"
        >
          <span class="font-mono text-[13px] font-medium text-ink">{{ wf.name }}</span>
          <span v-if="wf.description" class="text-xs font-light text-ink-secondary">{{ wf.description }}</span>
          <span class="font-mono text-[11px] text-ink-tertiary">
            {{ editableInputsOf(wf).length }} input(s) · {{ (wf.outputs || []).length }} output(s)
          </span>
        </button>

        <div v-if="loadingWorkflows" class="font-mono text-xs text-ink-tertiary px-1">loading workflows…</div>

        <button
          class="px-4 py-3 border border-dashed border-line-strong rounded text-[13px] text-ink-secondary hover:text-signal transition-colors text-left"
          @click="openImportForm"
        >+ Import workflow</button>
      </div>

      <!-- Import form -->
      <div v-if="showImportForm" class="card-ab p-6 flex flex-col gap-4">
        <div class="flex items-center justify-between">
          <div class="label">Import workflow</div>
          <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="showImportForm = false">✕</button>
        </div>
        <div class="flex flex-col gap-2">
          <span class="label">Name</span>
          <input
            v-model="importName"
            placeholder="upscale-4x"
            spellcheck="false"
            class="bg-base border border-line rounded-sm px-3 py-2 font-mono text-[13px] text-ink"
          />
        </div>
        <div class="flex flex-col gap-2">
          <span class="label">Description</span>
          <input
            v-model="importDescription"
            placeholder="Optional"
            spellcheck="false"
            class="bg-base border border-line rounded-sm px-3 py-2 text-[13px] text-ink"
          />
        </div>
        <div class="flex flex-col gap-2">
          <span class="label">Workflow JSON</span>
          <textarea
            v-model="importJson"
            rows="10"
            placeholder="Paste ComfyUI API-format JSON"
            spellcheck="false"
            class="w-full bg-base border border-line rounded-sm px-3 py-2 font-mono text-xs text-ink"
          ></textarea>
          <span class="text-xs font-light text-ink-secondary">
            Inputs (LoadImage, text fields) and outputs are detected automatically on import.
          </span>
        </div>
        <div v-if="importError" class="font-mono text-xs text-error">{{ importError }}</div>
        <div v-if="importSuccess" class="font-mono text-xs text-ready">workflow imported</div>
        <div>
          <button
            class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
            :disabled="importing || !importReady"
            @click="importWorkflow"
          >{{ importing ? 'Importing…' : 'Import' }}</button>
        </div>
      </div>

      <!-- Workflow detail -->
      <div v-else-if="selectedWorkflow" class="card-ab p-6 flex flex-col gap-6 min-w-0">
        <div class="flex items-start justify-between gap-4">
          <div class="min-w-0">
            <div class="font-mono text-base font-medium text-ink">{{ selectedWorkflow.name }}</div>
            <div v-if="selectedWorkflow.description" class="text-[13px] font-light text-ink-secondary mt-1">
              {{ selectedWorkflow.description }}
            </div>
            <div class="font-mono text-[11px] text-ink-tertiary mt-2">
              imported {{ formatDate(selectedWorkflow.created_at) }}
              <template v-if="(selectedWorkflow.outputs || []).length">
                · output {{ selectedWorkflow.outputs[0].node_type }}
              </template>
            </div>
          </div>
          <button
            class="border border-line-strong rounded px-3 py-1.5 text-xs text-error flex-none"
            @click="deleteWorkflow(selectedWorkflow.id)"
          >Delete</button>
        </div>

        <!-- Detected inputs -->
        <div class="flex flex-col gap-2 min-w-0">
          <div class="label">Detected inputs</div>
          <div class="border border-line rounded overflow-hidden overflow-x-auto">
            <div
              class="grid gap-4 px-3 py-2 border-b min-w-[520px]"
              style="grid-template-columns: 64px 1fr 1fr 72px 1fr; border-color: var(--border-strong)"
            >
              <span class="label">Node</span>
              <span class="label">Type</span>
              <span class="label">Field</span>
              <span class="label">Kind</span>
              <span class="label">Default</span>
            </div>
            <div
              v-for="input in (selectedWorkflow.inputs || [])"
              :key="`${input.node_id}.${input.field_name}`"
              class="grid gap-4 px-3 py-2 border-b border-line font-mono text-xs min-w-[520px]"
              style="grid-template-columns: 64px 1fr 1fr 72px 1fr"
            >
              <span class="text-ink-tertiary">{{ input.node_id }}</span>
              <span class="text-ink-secondary truncate">
                {{ input.node_type }}
                <span v-if="input.node_title" class="text-ink-tertiary">· {{ input.node_title }}</span>
              </span>
              <span class="text-ink truncate">{{ input.field_name }}</span>
              <span class="text-ink-tertiary uppercase">{{ input.widget?.kind || '—' }}</span>
              <span class="text-ink-secondary truncate">
                {{ input.node_type === 'LoadImage' ? '(source file)' : (input.current_value ?? '') }}
              </span>
            </div>
          </div>
        </div>

        <!-- Prompt presets -->
        <div class="flex flex-col gap-2">
          <div class="label">Prompt presets</div>
          <div v-if="loadingPresets" class="font-mono text-xs text-ink-tertiary">loading presets…</div>
          <div class="flex flex-col gap-2">
            <div
              v-for="preset in presets"
              :key="preset.id"
              class="flex flex-col gap-2 p-3 border border-line rounded bg-base"
            >
              <div class="flex items-center gap-2">
                <template v-if="editingPresetId === preset.id">
                  <input
                    v-model="editingPresetName"
                    spellcheck="false"
                    class="bg-surface border border-line rounded-sm px-2 py-1 text-[13px] text-ink"
                    @keydown.enter="savePresetName(preset)"
                  />
                  <button class="font-mono text-[11px] text-signal" @click="savePresetName(preset)">save</button>
                  <button class="font-mono text-[11px] text-ink-tertiary" @click="editingPresetId = null">cancel</button>
                </template>
                <template v-else>
                  <span class="text-[13px] font-medium text-signal">{{ preset.name }}</span>
                  <button
                    class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                    @click="startEditPreset(preset)"
                  >rename</button>
                </template>
                <span class="flex-1"></span>
                <button
                  class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                  @click="deletePreset(preset.id)"
                >remove</button>
              </div>

              <WorkflowInputControls
                v-if="editableInputsOf(selectedWorkflow).length"
                v-model:text-overrides="preset.text_overrides"
                v-model:parameters="preset.parameters"
                :inputs="selectedWorkflow.inputs || []"
                :loader-keys="(selectedWorkflow.loaders || []).map(l => `${l.node_id}.${l.field}`)"
                @dirty="savePresetValues(preset)"
              />
              <span v-else class="text-xs font-light text-ink-secondary">
                No editable inputs in this workflow — the preset runs node defaults.
              </span>
            </div>
          </div>

          <div class="flex gap-2">
            <input
              v-model="newPresetName"
              placeholder="New preset name…"
              spellcheck="false"
              class="flex-1 bg-base border border-line rounded-sm px-3 py-2 text-[13px] text-ink"
              @keydown.enter="createPreset"
            />
            <button
              class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-50"
              :disabled="!newPresetName.trim()"
              @click="createPreset"
            >Add preset</button>
          </div>
          <div class="text-xs font-light text-ink-secondary">
            Presets save a whole set of values — prompts, seeds, step counts, models — per
            workflow. Picking one in the Enhance dialog fills the controls; changing one there
            deselects it.
          </div>
        </div>
      </div>

      <div v-else class="card-ab p-6 font-mono text-xs text-ink-tertiary">
        select a workflow, or import one
      </div>
    </div>

    <!-- The graph: what actually runs, and in what order. Full width, because a
         node diagram is the widest thing on this page and the least useful when
         cropped. -->
    <WorkflowGraph
      v-if="activeTab === 'workflows' && selectedWorkflow && !showImportForm"
      :workflow-id="selectedWorkflow.id"
      :editable-node-ids="editableNodeIds"
    />

    <!-- Queue tab -->
    <div v-else class="flex flex-col gap-2">
      <div class="card-ab overflow-hidden overflow-x-auto" ref="taskListRef" @scroll="onTaskListScroll">
        <div
          class="grid gap-3 px-4 py-2 border-b min-w-[760px]"
          style="grid-template-columns: 52px minmax(0,1fr) 140px 76px 108px 108px; border-color: var(--border-strong)"
        >
          <span></span>
          <span class="label">Source</span>
          <span class="label">Workflow</span>
          <span class="label">Time</span>
          <span class="label">Status</span>
          <span></span>
        </div>
        <div
          v-for="task in tasks"
          :key="task.id"
          class="grid gap-3 items-center px-4 py-2 border-b border-line hover:bg-raised transition-colors min-w-[760px]"
          style="grid-template-columns: 52px minmax(0,1fr) 140px 76px 108px 108px"
        >
          <!-- What is being enhanced. A queue of ids tells you nothing about
               which photo is stuck; the frame does. -->
          <router-link
            v-if="task.shot_id"
            :to="{ name: 'shot-detail', params: { id: task.shot_id } }"
            class="block w-[52px] h-10 rounded-sm bg-raised border border-line overflow-hidden hover:border-signal transition-colors"
            :title="task.source_name || `shot/${shortId(task.shot_id)}`"
          >
            <img
              v-if="task.thumbnail_url"
              :src="task.thumbnail_url"
              class="w-full h-full object-cover"
              loading="lazy"
            />
            <span v-else class="w-full h-full flex items-center justify-center font-mono text-[10px] text-ink-tertiary">—</span>
          </router-link>
          <span v-else class="w-[52px] h-10 rounded-sm bg-raised border border-line"></span>

          <span class="min-w-0">
            <router-link
              v-if="task.shot_id"
              :to="{ name: 'shot-detail', params: { id: task.shot_id } }"
              class="block text-[13px] truncate hover:text-signal transition-colors"
              :class="task.person_name ? 'text-ink' : 'text-ink-tertiary'"
            >{{ task.person_name || 'unsorted' }}</router-link>
            <span v-else class="block text-[13px] text-ink-tertiary">shot deleted</span>
            <span class="block font-mono text-[11px] text-ink-tertiary truncate">
              {{ task.source_name || `shot/${shortId(task.shot_id)}` }}
            </span>
            <!-- Why it failed, said once, where the failure is. -->
            <span
              v-if="task.error_message"
              class="block font-mono text-[11px] truncate"
              style="color: var(--status-error)"
              :title="task.error_message"
            >{{ task.error_message }}</span>
          </span>

          <span class="font-mono text-xs text-ink truncate">{{ task.workflow_name || task.workflow_id }}</span>

          <span class="font-mono text-xs text-ink-tertiary">
            {{ formatRelativeTime(task.created_at) }}
          </span>

          <span class="flex flex-col gap-0.5">
            <span
              class="flex items-center gap-1.5 font-mono text-[11px] tracking-[0.08em] uppercase"
              :style="{ color: statusColor(task.status) }"
            >
              <span
                class="signal-dot"
                :class="{ 'signal-pulse': isInFlight(task.status) }"
                style="width:6px;height:6px"
                :style="{ background: statusColor(task.status) }"
              ></span>
              {{ statusLabel(task.status) }}
            </span>
            <span v-if="task.retry_count > 0" class="font-mono text-[10px] text-ink-tertiary">
              {{ task.retry_count }} {{ task.retry_count === 1 ? 'retry' : 'retries' }}
            </span>
          </span>

          <span class="flex gap-3 justify-end">
            <template v-if="isInFlight(task.status)">
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                @click="cancelTask(task.id)"
              >cancel</button>
            </template>
            <template v-else-if="task.status === 'failed' || task.status === 'cancelled'">
              <!-- A stopped job is worth both verbs: run it again, or clear it out. -->
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                @click="retryTask(task.id)"
              >retry</button>
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                @click="deleteTask(task.id)"
              >delete</button>
            </template>
            <template v-else>
              <router-link
                v-if="task.status === 'completed' && task.shot_id"
                :to="{ name: 'shot-detail', params: { id: task.shot_id } }"
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              >open</router-link>
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                @click="deleteTask(task.id)"
              >remove</button>
            </template>
          </span>
        </div>

        <div v-if="loadingTasks && tasks.length === 0" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
          loading queue…
        </div>
        <div v-else-if="tasks.length === 0" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
          nothing queued
        </div>
      </div>

      <button
        v-if="nextCursor"
        class="self-start border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-50"
        :disabled="loadingMore"
        @click="fetchMoreTasks"
      >{{ loadingMore ? 'Loading…' : 'Load more' }}</button>

      <div v-if="hasActiveTasks" class="font-mono text-[11px] text-ink-tertiary">
        refreshing every 5s while tasks are active
      </div>
    </div>
  </div>
</template>

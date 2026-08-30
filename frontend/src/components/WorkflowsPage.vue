<script setup>
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import WorkflowGraph from '@/components/WorkflowGraph.vue'
import WorkflowInputControls from '@/components/WorkflowInputControls.vue'
import WorkflowContract from '@/components/WorkflowContract.vue'
import LineEditor from '@/components/LineEditor.vue'
import { controlKind, isParameterInput, isTextInput, inputKey, parameterValue, formatDuration, stageOf, readinessColor, installedLabel } from '@/lib/utils'
import { typeTrack, continuationCost, heldLabel, takeSeed } from '@/lib/lines'

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

// ===== QUEUE TAB — the board shows runs, not tasks =====
//
// A four-stage line is four rows in `enhancement_tasks` and one thing the user
// asked for. So the board is a schedule of runs: one row each, saying which
// stage it is on, of how many, and what that stage is running. The tasks
// underneath stay reachable — a row opens to show them — but they are not the
// unit anybody reads.
const runs = ref([])
const loadingRuns = ref(false)
const loadingMore = ref(false)
const nextCursor = ref(null)
let taskRefreshInterval = null

/** The run whose tasks are open beneath it, and those tasks. */
const openRunId = ref(null)
const openRunTasks = ref([])

async function fetchRuns() {
  loadingRuns.value = true
  try {
    const res = await fetch('/api/comfyui/runs?limit=50')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    runs.value = data.items
    nextCursor.value = data.next_cursor
  } catch (e) {
    console.error('Failed to fetch runs', e)
  } finally {
    loadingRuns.value = false
  }
}

async function fetchMoreRuns() {
  if (!nextCursor.value || loadingMore.value) return
  loadingMore.value = true
  try {
    const res = await fetch(`/api/comfyui/runs?limit=50&cursor=${encodeURIComponent(nextCursor.value)}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    // Deduplicate by id in case polling shifted items
    const existingIds = new Set(runs.value.map(r => r.id))
    runs.value = [...runs.value, ...data.items.filter(r => !existingIds.has(r.id))]
    nextCursor.value = data.next_cursor
  } catch (e) {
    console.error('Failed to fetch more runs', e)
  } finally {
    loadingMore.value = false
  }
}

const taskListRef = ref(null)

function onTaskListScroll(event) {
  const el = event.target
  if (el.scrollHeight - el.scrollTop - el.clientHeight < 200) {
    fetchMoreRuns()
  }
}

const hasActiveRuns = computed(() => runs.value.some(r => r.status === 'running'))

/** The tasks under one run — the drill-down, fetched only when opened. */
async function fetchOpenRunTasks() {
  if (!openRunId.value) return
  try {
    const res = await fetch(`/api/comfyui/runs/${openRunId.value}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    openRunTasks.value = (await res.json()).tasks || []
  } catch (e) {
    console.error('Failed to fetch run tasks', e)
  }
}

async function toggleRun(runId) {
  if (openRunId.value === runId) {
    openRunId.value = null
    openRunTasks.value = []
    return
  }
  openRunId.value = runId
  openRunTasks.value = []
  await fetchOpenRunTasks()
}

function startTaskPolling() {
  stopTaskPolling()
  taskRefreshInterval = setInterval(() => {
    fetchRuns()
    fetchOpenRunTasks()
  }, 5000)
}

function stopTaskPolling() {
  if (taskRefreshInterval) {
    clearInterval(taskRefreshInterval)
    taskRefreshInterval = null
  }
}

async function runAction(runId, verb) {
  try {
    const res = await fetch(`/api/comfyui/runs/${runId}/${verb}`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchRuns()
    await fetchOpenRunTasks()
  } catch (e) {
    console.error(`Failed to ${verb} run`, e)
  }
}

const cancelRun = (id) => runAction(id, 'cancel')
/** Resumes from the stage that failed. What already succeeded is not re-run. */
const retryRun = (id) => runAction(id, 'retry')

async function retryTask(taskId) {
  try {
    const res = await fetch(`/api/comfyui/tasks/${taskId}/retry`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchRuns()
    await fetchOpenRunTasks()
  } catch (e) {
    console.error('Failed to retry task', e)
  }
}

async function cancelTask(taskId) {
  try {
    const res = await fetch(`/api/comfyui/tasks/${taskId}/cancel`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    await fetchRuns()
    await fetchOpenRunTasks()
  } catch (e) {
    console.error('Failed to cancel task', e)
  }
}

// ===== HOLD POINTS — the verdict, from the board =====
//
// Enough surface for a person to actually give a verdict, and no more: the
// takes of one run, tick the ones worth the stages below, and the three
// buttons. The curation lane — every held run at once, keyboard-driven, big
// pictures — is FR10b, and it reads these same two endpoints.
const openHoldId = ref(null)
const hold = ref(null)
const keptTakes = ref([])
const holdBusy = ref(false)
const holdError = ref('')

/** How many tasks continuing with what is ticked will queue. */
const holdCost = computed(() => continuationCost(keptTakes.value.length, hold.value?.tasks_per_take))

async function toggleHold(runId) {
  if (openHoldId.value === runId) {
    openHoldId.value = null
    hold.value = null
    return
  }
  openHoldId.value = runId
  hold.value = null
  keptTakes.value = []
  holdError.value = ''
  try {
    const res = await fetch(`/api/comfyui/runs/${runId}/hold`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    hold.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch hold', e)
    holdError.value = 'Could not read what this run is holding.'
  }
}

function toggleTake(taskId) {
  keptTakes.value = keptTakes.value.includes(taskId)
    ? keptTakes.value.filter(id => id !== taskId)
    : [...keptTakes.value, taskId]
}

async function giveVerdict(runId, verdict) {
  if (holdBusy.value) return
  holdBusy.value = true
  holdError.value = ''
  try {
    const res = await fetch(`/api/comfyui/runs/${runId}/hold`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ verdict, keep: verdict === 'continue' ? keptTakes.value : [] }),
    })
    const data = await res.json().catch(() => ({}))
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)
    openHoldId.value = null
    hold.value = null
    keptTakes.value = []
    await fetchRuns()
  } catch (e) {
    holdError.value = String(e.message || e)
  } finally {
    holdBusy.value = false
  }
}

// ===== LINES TAB =====
//
// Three ways to get a line, and blank is the last of them. Fork a template and
// change a stage; promote a sequence Phos has watched somebody run by hand;
// or, failing both, compose one. `LineEditor.vue` is the screen; everything
// here is what it is shown and where its answers go.
const lines = ref([])
const loadingLines = ref(false)
const selectedLineId = ref(null)
const composing = ref(false)
const lineError = ref('')

const selectedLine = computed(() =>
  lines.value.find((l) => l.id === selectedLineId.value) || null,
)

async function fetchLines() {
  loadingLines.value = true
  try {
    const res = await fetch('/api/comfyui/lines')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    lines.value = (await res.json()).items
  } catch (e) {
    console.error('Failed to fetch lines', e)
  } finally {
    loadingLines.value = false
  }
}

function selectLine(id) {
  composing.value = false
  selectedLineId.value = id
}

function startBlankLine() {
  selectedLineId.value = null
  composing.value = true
  lineError.value = ''
}

async function onLineSaved(saved) {
  composing.value = false
  await fetchLines()
  if (saved?.id) selectedLineId.value = saved.id
}

/** Fork: the default way to get a line, and the way to change a locked one. */
async function duplicateLine(id) {
  lineError.value = ''
  try {
    const res = await fetch(`/api/comfyui/lines/${id}/duplicate`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const fork = await res.json()
    await fetchLines()
    selectedLineId.value = fork.id
    composing.value = true
  } catch (e) {
    lineError.value = e.message || 'Could not duplicate the line'
  }
}

async function deleteLine(id) {
  if (!confirm('Delete this line?')) return
  try {
    const res = await fetch(`/api/comfyui/lines/${id}`, { method: 'DELETE' })
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    if (selectedLineId.value === id) selectedLineId.value = null
    await fetchLines()
  } catch (e) {
    lineError.value = e.message || 'Failed to delete line'
  }
}

// --- Lines travel ---
//
// A line lives in its library's own .phos.db, so without this there is no way
// to move one to another library, another install, or another person. Export
// is a download; import shows what the line needs before it commits anything.
// The editor proper is FR5b — this is deliberately two buttons.

async function exportLine(ln) {
  lineError.value = ''
  try {
    const res = await fetch(`/api/comfyui/lines/${ln.id}/export`)
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    const bundle = await res.json()
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(bundle, null, 2)], { type: 'application/json' }),
    )
    const a = document.createElement('a')
    a.href = url
    a.download = `${ln.name.replace(/[^\w.-]+/g, '-').toLowerCase() || 'line'}.phos-line.json`
    a.click()
    URL.revokeObjectURL(url)
  } catch (e) {
    lineError.value = e.message || 'Failed to export line'
  }
}

const showLineImport = ref(false)
const importLineJson = ref('')
const importLineName = ref('')
const importLineReport = ref(null)
const importLineError = ref('')
const importingLine = ref(false)

function openLineImport() {
  showLineImport.value = true
  importLineJson.value = ''
  importLineName.value = ''
  importLineReport.value = null
  importLineError.value = ''
}

async function onLineFile(event) {
  const file = event.target.files?.[0]
  if (!file) return
  importLineJson.value = await file.text()
  // A file the person picked is a file they mean; check it straight away.
  await checkLineImport()
}

/** POST the bundle with `dry_run`, so the report is on screen before anything
 *  is written. This is the whole point of the dialog. */
async function checkLineImport() {
  return sendLineImport(true)
}

async function sendLineImport(dryRun) {
  importLineError.value = ''
  let bundle
  try {
    bundle = JSON.parse(importLineJson.value)
  } catch {
    importLineError.value = 'That is not JSON.'
    importLineReport.value = null
    return
  }

  importingLine.value = true
  try {
    const params = new URLSearchParams()
    if (dryRun) params.set('dry_run', 'true')
    if (importLineName.value.trim()) params.set('name', importLineName.value.trim())
    const res = await fetch(`/api/comfyui/lines/import?${params}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(bundle),
    })
    const data = await res.json().catch(() => ({}))
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)
    importLineReport.value = data
    if (!dryRun) {
      showLineImport.value = false
      await fetchLines()
    }
  } catch (e) {
    importLineError.value = e.message || 'Failed to import line'
    if (dryRun) importLineReport.value = null
  } finally {
    importingLine.value = false
  }
}

/** ready / missing / unchecked — the only three, and the only three colours. */
function reportColor(status) {
  switch (status) {
    case 'ready': return 'var(--status-ready)'
    case 'missing': return 'var(--status-error)'
    default: return 'var(--status-degraded)'
  }
}


// --- Promote from history -------------------------------------------------
// Nobody sits down to design a four-stage chain; they run three workflows in a
// row on twelve shots. Phos already recorded which file each run read, so the
// sequence is written down and the only thing missing is somebody noticing.
const suggestions = ref([])
const dismissed = ref(new Set())

async function fetchSuggestions() {
  try {
    const res = await fetch('/api/comfyui/lines/suggestions')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    suggestions.value = (await res.json()).items
  } catch (e) {
    // Nothing to say: a suggestion nobody asked for that cannot be made is
    // not worth a message.
    console.warn('Could not read line suggestions', e)
    suggestions.value = []
  }
}

const liveSuggestions = computed(() =>
  suggestions.value.filter((s) => !dismissed.value.has(s.name)),
)

function dismissSuggestion(name) {
  dismissed.value = new Set([...dismissed.value, name])
}

async function acceptSuggestion(suggestion) {
  lineError.value = ''
  try {
    const res = await fetch('/api/comfyui/lines', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: suggestion.name,
        description: 'Promoted from what you have been running by hand.',
        stages: suggestion.stages.map((s) => ({
          workflow_id: s.workflow_id,
          text_overrides: s.text_overrides || {},
          parameters: s.parameters || {},
          vary: {},
          source_mode: s.source_mode ?? null,
          keep_output: false,
          exposed: s.exposed || [],
        })),
      }),
    })
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    const saved = await res.json()
    dismissSuggestion(suggestion.name)
    await Promise.all([fetchLines(), fetchSuggestions()])
    selectedLineId.value = saved.id
    composing.value = false
  } catch (e) {
    lineError.value = e.message || 'Could not save that line'
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

/**
 * A run has five states, not nine: it is walking its line, it is parked at a
 * hold waiting for a person, or it is however it ended. The nine belong to the
 * tasks underneath.
 *
 * `held` gets the degraded amber — the colour this system already uses for
 * "this needs somebody", which is exactly what a hold is. It is not an error
 * and it is not progress.
 */
function runStatusColor(status) {
  switch (status) {
    case 'completed': return 'var(--status-ready)'
    case 'failed': return 'var(--status-error)'
    case 'cancelled': return 'var(--status-stopped)'
    case 'held': return 'var(--status-degraded)'
    default: return 'var(--status-building)'
  }
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

// ===== TEMPLATES TAB =====
//
// The five lines that ship with Phos. Seeding happens at startup, so this tab
// is mostly a *report*: what came with the build, what this library has of it,
// and — the part that earns the screen — whether this ComfyUI can actually run
// it. A template that cannot run names exactly what to install, here, rather
// than failing at dispatch some minutes into a run.
const templates = ref([])
const loadingTemplates = ref(false)
const catalogAvailable = ref(true)
const installingKey = ref(null)
const templateError = ref('')
const openTemplateKey = ref(null)

async function fetchTemplates() {
  loadingTemplates.value = true
  try {
    const res = await fetch('/api/comfyui/templates')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    templates.value = data.items
    catalogAvailable.value = data.catalog_available
  } catch (e) {
    console.error('Failed to fetch templates', e)
  } finally {
    loadingTemplates.value = false
  }
}

async function installTemplate(key) {
  installingKey.value = key
  templateError.value = ''
  try {
    const res = await fetch(`/api/comfyui/templates/${key}/install`, { method: 'POST' })
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    // The template wrote workflows and a line, so both lists are now stale.
    await Promise.all([fetchTemplates(), fetchWorkflows(), fetchLines()])
  } catch (e) {
    console.error('Failed to install template', e)
    templateError.value = e.message
  } finally {
    installingKey.value = null
  }
}

// --- Active tab tracking (synced with URL) ---
const route = useRoute()
const router = useRouter()
const TABS = ['templates', 'workflows', 'lines', 'queue']
const activeTab = computed(() => TABS.includes(route.query.tab) ? route.query.tab : 'workflows')

function onTabChange(val) {
  router.replace({ query: { ...route.query, tab: val === 'workflows' ? undefined : val } })
  if (val === 'queue') {
    fetchRuns()
    startTaskPolling()
  } else {
    stopTaskPolling()
    if (val === 'lines') {
      fetchLines()
      fetchSuggestions()
    }
    if (val === 'templates') fetchTemplates()
  }
}

// --- Lifecycle ---
onMounted(() => {
  checkHealth()
  fetchWorkflows()
  fetchLines()
  fetchSuggestions()
  if (activeTab.value === 'templates') fetchTemplates()
  if (activeTab.value === 'queue') {
    fetchRuns()
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
          { id: 'templates', label: 'Templates', count: templates.length },
          { id: 'workflows', label: 'Workflows', count: workflows.length },
          { id: 'lines', label: 'Lines', count: lines.length },
          { id: 'queue', label: 'Queue', count: runs.length },
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
          <span class="font-mono text-[11px] text-ink uppercase tracking-[0.08em]">
            {{ wf.contract?.accepts || '?' }} → {{ wf.contract?.produces || '?' }}
          </span>
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

    <!-- What this workflow is, before what it does: a line can only be built
         out of what each stage takes and gives back. -->
    <WorkflowContract
      v-if="activeTab === 'workflows' && selectedWorkflow && !showImportForm"
      :workflow-id="selectedWorkflow.id"
      :workflow-name="selectedWorkflow.name"
      :contract="selectedWorkflow.contract"
      @updated="fetchWorkflows"
    />

    <!-- The graph: what actually runs, and in what order. Full width, because a
         node diagram is the widest thing on this page and the least useful when
         cropped. -->
    <WorkflowGraph
      v-if="activeTab === 'workflows' && selectedWorkflow && !showImportForm"
      :workflow-id="selectedWorkflow.id"
      :editable-node-ids="editableNodeIds"
    />

    <!-- Templates tab — the five lines that ship with Phos.
         The status pill is the point of the screen: a template that cannot run
         on this ComfyUI says exactly what is missing here, rather than failing
         at dispatch some minutes into a run. `unknown` is its own answer — the
         catalogue could not be read, which is not evidence of a problem. -->
    <div v-else-if="activeTab === 'templates'" class="flex flex-col gap-3">
      <div class="text-[13px] font-light text-ink-secondary max-w-[620px]">
        Five ready-made lines. Installing one adds its workflows and its line to this
        library, where they are as editable as anything you imported yourself — and
        Phos stops updating one the moment you change it.
      </div>

      <div
        v-if="!catalogAvailable"
        class="card-ab px-4 py-3 font-mono text-[11px] tracking-[0.08em] uppercase"
        :style="{ color: readinessColor('unknown') }"
      >
        ComfyUI could not be asked what it has installed — readiness is unknown, not missing
      </div>

      <div v-if="templateError" class="font-mono text-xs text-error">{{ templateError }}</div>

      <div class="card-ab overflow-hidden">
        <div
          v-for="t in templates"
          :key="t.key"
          class="px-4 py-3 border-b border-line last:border-b-0 flex flex-col gap-2"
        >
          <div class="flex items-start gap-3">
            <span class="min-w-0 flex-1">
              <span class="block text-[13px] text-ink">{{ t.name }}</span>
              <span class="block font-mono text-[11px] tracking-[0.08em] uppercase text-ink-tertiary">
                {{ t.accepts }} → {{ t.produces }} ·
                {{ t.stage_count }} {{ t.stage_count === 1 ? 'stage' : 'stages' }} ·
                v{{ t.version }}<template v-if="t.confidence === 'unverified'"> · unverified</template>
              </span>
              <span class="block text-[13px] font-light text-ink-secondary mt-1">{{ t.summary }}</span>
            </span>

            <span
              class="flex items-center gap-1.5 font-mono text-[11px] tracking-[0.08em] uppercase whitespace-nowrap"
              :style="{ color: readinessColor(t.readiness.state) }"
            >
              <span
                class="signal-dot"
                style="width:6px;height:6px"
                :style="{ background: readinessColor(t.readiness.state) }"
              ></span>
              {{ t.readiness.label }}
            </span>

            <button
              class="border border-line-strong rounded px-3 py-1.5 text-[13px] text-ink-secondary hover:text-signal transition-colors whitespace-nowrap disabled:opacity-50"
              :disabled="installingKey === t.key"
              @click="installTemplate(t.key)"
            >{{ installingKey === t.key ? 'Installing…' : t.installed ? 'Install again' : 'Install' }}</button>
          </div>

          <div class="flex items-center gap-3">
            <span class="font-mono text-[11px] tracking-[0.08em] uppercase text-ink-tertiary">
              {{ installedLabel(t) }}
            </span>
            <span class="flex-1"></span>
            <button
              class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              @click="openTemplateKey = openTemplateKey === t.key ? null : t.key"
            >{{ openTemplateKey === t.key ? 'hide' : 'what it needs' }}</button>
          </div>

          <!-- What to install, and what to know before running it. -->
          <div v-if="openTemplateKey === t.key" class="flex flex-col gap-2 pt-1">
            <div
              class="text-[13px] font-light"
              :style="{ color: t.readiness.state === 'ready' ? undefined : readinessColor(t.readiness.state) }"
            >{{ t.readiness.detail }}</div>
            <div v-if="t.notes" class="text-[13px] font-light text-ink-secondary">{{ t.notes }}</div>
            <div class="flex flex-col gap-1">
              <span class="label">Nodes</span>
              <span class="font-mono text-[11px] text-ink-tertiary break-words">
                {{ t.requirements.node_classes.join(' · ') }}
              </span>
            </div>
            <div v-if="t.requirements.models.length" class="flex flex-col gap-1">
              <span class="label">Models</span>
              <span
                v-for="m in t.requirements.models"
                :key="m.class_type + m.field + m.name"
                class="font-mono text-[11px] text-ink-tertiary break-words"
              >{{ m.name }} → {{ m.class_type }}.{{ m.field }}</span>
            </div>
          </div>
        </div>

        <div v-if="loadingTemplates && templates.length === 0" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
          loading templates…
        </div>
        <div v-else-if="templates.length === 0" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
          no templates in this build
        </div>
      </div>
    </div>

    <!-- Lines tab — a chain of workflows run as one thing.
         Three ways in, and blank is the last of them: fork one, promote a
         sequence you have already been running by hand, or compose. -->
    <div v-else-if="activeTab === 'lines'" class="flex flex-col gap-4">
      <div class="flex flex-wrap items-baseline gap-4">
        <div class="text-[13px] font-light text-ink-secondary max-w-[560px]">
          A line runs its workflows in order, each stage reading what the one before it made.
          Start one from a shot's <span class="font-normal text-ink">Enhance</span> dialog.
        </div>
        <span class="flex-1"></span>
        <button
          v-if="!showLineImport"
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors whitespace-nowrap"
          @click="openLineImport"
        >Import line</button>
        <button
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors whitespace-nowrap disabled:opacity-50"
          :disabled="workflows.length === 0"
          @click="startBlankLine"
        >New line</button>
      </div>

      <!-- Import a line from a file. The requirements report comes first: a
           line naming a node this ComfyUI does not have has to say so here,
           not four stages into a run. -->
      <div v-if="showLineImport" class="card-ab p-4 flex flex-col gap-3">
        <div class="flex items-center gap-3">
          <span class="label w-24 flex-none">Bundle</span>
          <input
            type="file"
            accept="application/json,.json"
            class="flex-1 min-w-0 font-mono text-xs text-ink-secondary file:mr-3 file:border file:border-line-strong file:rounded-sm file:bg-base file:px-3 file:py-1 file:font-mono file:text-[11px] file:text-ink-secondary"
            @change="onLineFile"
          />
        </div>
        <div class="flex items-center gap-3">
          <span class="label w-24 flex-none">Name as</span>
          <input
            v-model="importLineName"
            placeholder="leave blank to keep the file's name"
            class="flex-1 min-w-0 bg-base border border-line rounded-sm px-3 py-1.5 text-[13px] text-ink"
          />
        </div>

        <div v-if="importLineError" class="font-mono text-xs" style="color: var(--status-error)">
          {{ importLineError }}
        </div>

        <!-- What this box can and cannot run. -->
        <div
          v-if="importLineReport"
          class="border rounded-sm p-3 flex flex-col gap-2"
          :style="{ borderColor: reportColor(importLineReport.report_status) }"
        >
          <div class="flex items-center gap-2 flex-wrap">
            <span class="tag" :style="{ color: reportColor(importLineReport.report_status) }">
              {{ importLineReport.report_status }}
            </span>
            <span class="font-mono text-[11px] tracking-[0.08em] uppercase text-ink-tertiary">
              {{ importLineReport.name }} · {{ importLineReport.stage_count }}
              {{ importLineReport.stage_count === 1 ? 'stage' : 'stages' }}
            </span>
          </div>
          <div class="text-[13px] text-ink-secondary">{{ importLineReport.report_headline }}</div>

          <div v-if="importLineReport.report?.missing_nodes?.length" class="flex flex-col gap-1">
            <span class="label">Missing nodes</span>
            <span class="font-mono text-[11px]" style="color: var(--status-error)">
              {{ importLineReport.report.missing_nodes.join(', ') }}
            </span>
          </div>
          <div v-if="importLineReport.report?.missing_models?.length" class="flex flex-col gap-1">
            <span class="label">Missing models</span>
            <span
              v-for="m in importLineReport.report.missing_models"
              :key="`${m.class_type}.${m.field}.${m.name}`"
              class="font-mono text-[11px]"
              style="color: var(--status-error)"
            >{{ m.name }} <span class="text-ink-tertiary">({{ m.class_type }}.{{ m.field }})</span></span>
          </div>
          <div
            v-for="w in importLineReport.report?.warnings || []"
            :key="w"
            class="font-mono text-[11px]"
            style="color: var(--status-degraded)"
          >{{ w }}</div>

          <div v-if="importLineReport.workflows?.length" class="flex flex-col gap-1">
            <span class="label">Workflows</span>
            <span
              v-for="wf in importLineReport.workflows"
              :key="wf.key"
              class="font-mono text-[11px] text-ink-tertiary"
            >{{ wf.name }} · {{ wf.reused ? `reuses ${wf.reused_as}` : 'new' }}</span>
          </div>
          <div
            v-if="importLineReport.renamed_from"
            class="font-mono text-[11px] text-ink-tertiary"
          >
            "{{ importLineReport.renamed_from }}" is taken, so this comes in as
            "{{ importLineReport.name }}".
          </div>
        </div>

        <div class="flex items-center gap-3">
          <button
            class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
            :disabled="!importLineJson || importingLine"
            @click="sendLineImport(false)"
          >{{ importingLine ? 'Working…' : 'Import' }}</button>
          <button
            class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
            :disabled="!importLineJson || importingLine"
            @click="checkLineImport"
          >re-check</button>
          <span class="flex-1"></span>
          <button
            class="font-mono text-[11px] text-ink-tertiary hover:text-ink transition-colors"
            @click="showLineImport = false"
          >cancel</button>
        </div>
      </div>

      <!-- Promoted from history: the sequence somebody has already been
           running one workflow at a time, offered as the line it is. -->
      <div
        v-for="s in liveSuggestions"
        :key="s.name"
        class="card-ab p-4 flex flex-col gap-2"
        style="border-color: var(--accent-muted)"
      >
        <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--accent)"></span>
          <span class="label">Noticed</span>
          <span class="text-[13px] text-ink">
            You ran
            <span class="font-mono">{{ s.stages.map(st => st.workflow_name || st.workflow_id).join(' → ') }}</span>
            on {{ s.shot_count }} shots.
          </span>
          <span class="flex-1"></span>
          <button
            class="font-mono text-[11px] text-signal hover:underline underline-offset-2"
            @click="acceptSuggestion(s)"
          >save as a line</button>
          <button
            class="font-mono text-[11px] text-ink-tertiary hover:text-ink transition-colors"
            @click="dismissSuggestion(s.name)"
          >dismiss</button>
        </div>
        <div
          v-if="s.stages.some(st => (st.exposed || []).length)"
          class="font-mono text-[11px] text-ink-tertiary"
        >
          The settings you changed every time will be asked for when the line is sent; the ones
          you never changed are part of it.
        </div>
      </div>

      <div v-if="lineError" class="font-mono text-xs" style="color: var(--status-error)">{{ lineError }}</div>

      <div class="grid gap-6 items-start lg:grid-cols-[280px_minmax(0,1fr)]">
        <!-- The lines this library holds. -->
        <div class="flex flex-col gap-2">
          <button
            v-for="ln in lines"
            :key="ln.id"
            class="flex flex-col gap-1 p-3 border rounded text-left transition-colors"
            :class="selectedLineId === ln.id ? 'border-signal bg-surface' : 'border-line hover:bg-raised'"
            @click="selectLine(ln.id)"
          >
            <span class="flex items-baseline gap-2 min-w-0">
              <span class="font-mono text-[13px] font-medium text-ink truncate">{{ ln.name }}</span>
              <span class="flex-1"></span>
              <span
                v-if="!ln.valid"
                class="signal-dot"
                style="width:6px;height:6px;background:var(--status-error)"
                title="This chain no longer fits together"
              ></span>
              <span
                v-else-if="ln.live_runs"
                class="signal-dot signal-pulse"
                style="width:6px;height:6px;background:var(--status-building)"
                title="A run is walking this line"
              ></span>
            </span>
            <span class="font-mono text-[11px] text-ink-tertiary truncate">
              {{ ln.stages.map(st => st.workflow_name).join(' → ') }}
            </span>
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary">
              {{ ln.stage_count }} {{ ln.stage_count === 1 ? 'stage' : 'stages' }} · {{ typeTrack(ln) }}
            </span>
          </button>

          <div v-if="loadingLines && lines.length === 0" class="font-mono text-xs text-ink-tertiary px-1">
            loading lines…
          </div>
          <div v-else-if="lines.length === 0" class="font-mono text-xs text-ink-tertiary px-1">
            no lines yet
          </div>
        </div>

        <!-- The editor. Read-only it draws as a route board; under edit it is a
             list, and Add stage only offers what fits where it is going. -->
        <LineEditor
          v-if="selectedLine || composing"
          :key="`${selectedLineId || 'new'}-${composing}`"
          :line="selectedLine"
          :workflows="workflows"
          :start-editing="composing"
          @saved="onLineSaved"
          @cancelled="composing = false"
          @duplicate="duplicateLine(selectedLineId)"
          @export="exportLine(selectedLine)"
          @delete="deleteLine(selectedLineId)"
        />
        <div v-else class="card-ab p-6 font-mono text-xs text-ink-tertiary">
          select a line, or start one
        </div>
      </div>
    </div>

    <!-- Queue tab — a schedule of runs.
         A four-stage run is one row that says which stage it is on, not four
         unrelated ones. The tasks underneath open on click. -->
    <div v-else-if="activeTab === 'queue'" class="flex flex-col gap-2">
      <div class="card-ab overflow-hidden overflow-x-auto" ref="taskListRef" @scroll="onTaskListScroll">
        <div
          class="grid gap-3 px-4 py-2 border-b min-w-[860px]"
          style="grid-template-columns: 52px minmax(0,1fr) 150px 168px 84px 108px 92px; border-color: var(--border-strong)"
        >
          <span></span>
          <span class="label">Shot</span>
          <span class="label">Line</span>
          <span class="label">Stage</span>
          <span class="label">Clock</span>
          <span class="label">Status</span>
          <span></span>
        </div>

        <template v-for="run in runs" :key="run.id">
          <div
            class="grid gap-3 items-center px-4 py-2 border-b border-line hover:bg-raised transition-colors min-w-[860px]"
            style="grid-template-columns: 52px minmax(0,1fr) 150px 168px 84px 108px 92px"
          >
            <!-- What is being worked on. A queue of ids tells you nothing about
                 which photo is stuck; the frame does. -->
            <router-link
              v-if="run.shot_id"
              :to="{ name: 'shot-detail', params: { id: run.shot_id } }"
              class="block w-[52px] h-10 rounded-sm bg-raised border border-line overflow-hidden hover:border-signal transition-colors"
              :title="run.source_name || `shot/${shortId(run.shot_id)}`"
            >
              <img
                v-if="run.thumbnail_url"
                :src="run.thumbnail_url"
                class="w-full h-full object-cover"
                loading="lazy"
              />
              <span v-else class="w-full h-full flex items-center justify-center font-mono text-[10px] text-ink-tertiary">—</span>
            </router-link>
            <span v-else class="w-[52px] h-10 rounded-sm bg-raised border border-line"></span>

            <span class="min-w-0">
              <router-link
                v-if="run.shot_id"
                :to="{ name: 'shot-detail', params: { id: run.shot_id } }"
                class="block text-[13px] truncate hover:text-signal transition-colors"
                :class="run.person_name ? 'text-ink' : 'text-ink-tertiary'"
              >{{ run.person_name || 'unsorted' }}</router-link>
              <span class="block font-mono text-[11px] text-ink-tertiary truncate">
                {{ run.source_name || `shot/${shortId(run.shot_id)}` }}
              </span>
              <!-- Why it stopped, said once, on the run that stopped. -->
              <span
                v-if="run.error_message"
                class="block font-mono text-[11px] truncate"
                style="color: var(--status-error)"
                :title="run.error_message"
              >{{ run.error_message }}</span>
            </span>

            <span class="font-mono text-xs text-ink truncate uppercase tracking-[0.04em]">{{ run.label }}</span>

            <!-- The one thing a task queue could never say: how far along the
                 chain this is, and what it is doing right now. -->
            <span class="flex items-baseline gap-2 min-w-0">
              <span class="font-mono text-[11px] tracking-[0.08em] uppercase text-ink-secondary whitespace-nowrap">
                stage {{ stageOf(run) }}
              </span>
              <span class="font-mono text-[11px] text-ink-tertiary truncate">{{ run.stage_label || '' }}</span>
              <span v-if="run.in_flight > 1" class="font-mono text-[10px] text-signal whitespace-nowrap">×{{ run.in_flight }}</span>
            </span>

            <span class="font-mono text-xs text-ink-tertiary tabular-nums">{{ formatDuration(run.elapsed_seconds) }}</span>

            <!-- A held run says what it is waiting on, because the number is
                 the whole decision: HELD · 4 TAKES. -->
            <span
              class="flex items-center gap-1.5 font-mono text-[11px] tracking-[0.08em] uppercase"
              :style="{ color: runStatusColor(run.status) }"
            >
              <span
                class="signal-dot"
                :class="{ 'signal-pulse': run.status === 'running' }"
                style="width:6px;height:6px"
                :style="{ background: runStatusColor(run.status) }"
              ></span>
              {{ run.status === 'held' ? heldLabel(run) : run.status }}
            </span>

            <span class="flex gap-3 justify-end">
              <button
                v-if="run.status === 'held'"
                class="font-mono text-[11px] transition-colors"
                :style="{ color: 'var(--status-degraded)' }"
                title="Look at this run's takes and say which of them are worth the stages below."
                @click="toggleHold(run.id)"
              >{{ openHoldId === run.id ? 'close' : 'review' }}</button>
              <button
                v-else-if="run.status === 'running'"
                class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                @click="cancelRun(run.id)"
              >cancel</button>
              <button
                v-else-if="run.status === 'failed' || run.status === 'cancelled'"
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                title="Resumes from the stage that stopped. What already succeeded is not re-run."
                @click="retryRun(run.id)"
              >resume</button>
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                @click="toggleRun(run.id)"
              >{{ openRunId === run.id ? 'hide' : 'stages' }}</button>
            </span>
          </div>

          <!-- The takes, and the three things a person can say about them.
               Curation as a step inside the pipeline rather than a bin at the
               end of it. -->
          <div
            v-if="openHoldId === run.id"
            class="px-4 py-3 border-b bg-base min-w-[860px] flex flex-col gap-3"
            style="border-color: var(--status-degraded)"
          >
            <div class="flex flex-wrap items-baseline gap-x-4 gap-y-1">
              <span class="label" style="color: var(--status-degraded)">
                Held at stage {{ (hold?.stage_idx ?? 0) + 1 }} / {{ hold?.stage_count ?? run.stage_count }}
              </span>
              <span class="font-mono text-[11px] text-ink-secondary">{{ hold?.stage_label || run.stage_label || '' }}</span>
              <span class="font-mono text-[11px] text-ink-tertiary">
                {{ (hold?.takes || []).length }} take(s) waiting · nothing below this stage has run
              </span>
            </div>

            <div v-if="holdError" class="font-mono text-[11px]" style="color: var(--status-error)">{{ holdError }}</div>
            <div v-if="!hold && !holdError" class="font-mono text-[11px] text-ink-tertiary">loading takes…</div>

            <div v-if="hold" class="flex flex-wrap gap-2">
              <button
                v-for="take in hold.takes"
                :key="take.task_id"
                class="w-[132px] text-left border rounded-sm overflow-hidden bg-surface transition-colors"
                :style="{ borderColor: keptTakes.includes(take.task_id) ? 'var(--accent)' : 'var(--border)' }"
                @click="toggleTake(take.task_id)"
              >
                <span class="block w-full h-[88px] bg-raised">
                  <img
                    v-if="take.thumbnail_url"
                    :src="take.thumbnail_url"
                    class="w-full h-full object-cover"
                    loading="lazy"
                  />
                  <!-- A describe stage's take is a sentence, and there is
                       nothing to photograph. -->
                  <span
                    v-else
                    class="block w-full h-full p-2 font-mono text-[10px] leading-tight text-ink-secondary overflow-hidden"
                  >{{ take.text_output || '—' }}</span>
                </span>
                <span class="flex items-center justify-between gap-1 px-2 py-1 border-t border-line">
                  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary truncate">
                    <template v-if="takeSeed(take) !== null">seed {{ takeSeed(take) }}</template>
                    <template v-else>{{ shortId(take.task_id) }}</template>
                  </span>
                  <span
                    class="signal-dot flex-none"
                    style="width:6px;height:6px"
                    :style="{ background: keptTakes.includes(take.task_id) ? 'var(--accent)' : 'var(--border-strong)' }"
                  ></span>
                </span>
              </button>
            </div>

            <div v-if="hold" class="flex flex-wrap items-center gap-4">
              <button
                class="border rounded-sm px-4 py-1.5 font-mono text-[11px] uppercase tracking-[0.08em] transition-colors disabled:opacity-40"
                style="border-color: var(--accent); color: var(--accent)"
                :disabled="holdBusy || !keptTakes.length"
                @click="giveVerdict(run.id, 'continue')"
              >continue</button>
              <!-- The cost moves as boxes are ticked. That is the reason to
                   stop here at all. -->
              <span class="font-mono text-[11px] text-ink-tertiary">
                {{ keptTakes.length }} kept → {{ holdCost }} task(s) below
              </span>
              <span class="flex-1"></span>
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors disabled:opacity-40"
                :disabled="holdBusy"
                title="Run this stage again with fresh seeds. Same prompt, same parameters, same source."
                @click="giveVerdict(run.id, 'regenerate')"
              >regenerate</button>
              <button
                class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors disabled:opacity-40"
                :disabled="holdBusy"
                title="Abandon the run. Intermediates go, except where a stage says keep."
                @click="giveVerdict(run.id, 'cancel')"
              >abandon</button>
            </div>
          </div>

          <!-- The tasks underneath: reachable, but not the top-level unit. -->
          <div
            v-if="openRunId === run.id"
            class="px-4 py-2 border-b border-line bg-base min-w-[860px]"
          >
            <div
              v-for="task in openRunTasks"
              :key="task.id"
              class="flex items-center gap-3 py-1"
            >
              <span class="label w-16 flex-none">St {{ (task.stage_idx ?? 0) + 1 }}</span>
              <span class="font-mono text-[11px] text-ink-secondary w-40 flex-none truncate">{{ task.workflow_name }}</span>
              <span
                class="flex items-center gap-1.5 font-mono text-[11px] tracking-[0.08em] uppercase w-32 flex-none"
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
              <span class="font-mono text-[11px] text-ink-tertiary flex-1 min-w-0 truncate" :title="task.error_message">
                {{ task.error_message || '' }}
              </span>
              <button
                v-if="isInFlight(task.status)"
                class="font-mono text-[11px] text-ink-tertiary hover:text-error transition-colors"
                @click="cancelTask(task.id)"
              >cancel</button>
              <button
                v-else-if="task.status === 'failed' || task.status === 'cancelled'"
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
                @click="retryTask(task.id)"
              >retry</button>
              <router-link
                v-else-if="task.output_file_id"
                :to="{ name: 'shot-detail', params: { id: run.shot_id } }"
                class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              >open</router-link>
              <span v-else class="font-mono text-[11px] text-ink-tertiary">discarded</span>
            </div>
            <div v-if="openRunTasks.length === 0" class="py-1 font-mono text-[11px] text-ink-tertiary">
              no tasks — this run's steps have been swept
            </div>
          </div>
        </template>

        <div v-if="loadingRuns && runs.length === 0" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
          loading queue…
        </div>
        <div v-else-if="runs.length === 0" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
          nothing queued
        </div>
      </div>

      <button
        v-if="nextCursor"
        class="self-start border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-50"
        :disabled="loadingMore"
        @click="fetchMoreRuns"
      >{{ loadingMore ? 'Loading…' : 'Load more' }}</button>

      <div v-if="hasActiveRuns" class="font-mono text-[11px] text-ink-tertiary">
        refreshing every 5s while runs are active
      </div>
    </div>

  </div>
</template>

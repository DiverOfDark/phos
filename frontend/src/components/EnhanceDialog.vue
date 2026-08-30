<script setup>
import { ref, computed, watch } from 'vue'
import WorkflowInputControls from '@/components/WorkflowInputControls.vue'
import { isTextInput, isParameterInput, inputKey, parameterValue, runCount, MAX_FANOUT } from '@/lib/utils'

const props = defineProps({
  open: Boolean,
  shotId: [String, Number],
  shotLabel: { type: String, default: '' },
  fileId: String,
  /** Mime type of the file the run will read, so the source picker knows
   *  whether there is a video to take a frame of. */
  sourceMime: { type: String, default: '' },
})

const emit = defineEmits(['update:open', 'taskCreated'])

const dialogOpen = computed({
  get: () => props.open,
  set: (val) => emit('update:open', val),
})

// --- Workflows ---
const workflows = ref([])
const loadingWorkflows = ref(false)
const selectedWorkflowId = ref(null)

const selectedWorkflow = computed(() =>
  workflows.value.find(w => w.id === selectedWorkflowId.value) || null
)

// --- Presets ---
const presets = ref([])
const selectedPresetId = ref(null)

// --- Generations (existing variations for this shot) ---
const generations = ref([])

// --- What this run sets ---
// Two channels, matching the backend: prompts (and anything ComfyUI could not
// describe) as strings, everything else typed. `vary` turns one of those into
// several runs.
const textOverrides = ref({})
const parameters = ref({})
const vary = ref({})

// --- Source mode (videos only) ---
// A still has no frames to choose between, so the whole section stays out of
// the way unless the source is a video.
const sourceIsVideo = computed(() => (props.sourceMime || '').startsWith('video/'))

const SOURCE_MODES = [
  { key: 'whole_video', label: 'whole video', note: 'the clip itself — needs a workflow with a video loader' },
  { key: 'first_frame', label: 'first frame', note: 'frame zero' },
  { key: 'last_frame', label: 'last frame', note: 'what an extension continues from' },
  { key: 'at_time', label: 'at time', note: 'a position in the clip' },
  { key: 'keyframe', label: 'keyframe', note: 'one of the indexed keyframes' },
]

const sourceModeKey = ref('first_frame')
const sourceAtMs = ref(0)
const sourceKeyframe = ref(0)
const sourceModeTouched = ref(false)

/** What goes on the wire, or null to let the backend decide. */
const sourceMode = computed(() => {
  if (!sourceIsVideo.value) return null
  if (sourceModeKey.value === 'at_time') return `at_time:${Math.max(0, Math.trunc(sourceAtMs.value || 0))}`
  if (sourceModeKey.value === 'keyframe') return `keyframe:${Math.max(0, Math.trunc(sourceKeyframe.value || 0))}`
  return sourceModeKey.value
})

/** The default the backend would pick, mirrored so the UI shows the truth. */
function defaultSourceModeKey() {
  return selectedWorkflow.value?.takes_video ? 'whole_video' : 'first_frame'
}

function selectSourceMode(key) {
  sourceModeKey.value = key
  sourceModeTouched.value = true
}

// --- Submit state ---
const submitting = ref(false)
const submitError = ref('')
const submitSuccess = ref(false)

async function fetchWorkflows() {
  loadingWorkflows.value = true
  try {
    const res = await fetch('/api/comfyui/workflows')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    workflows.value = await res.json()
    if (workflows.value.length && !selectedWorkflowId.value) {
      selectedWorkflowId.value = workflows.value[0].id
    }
  } catch (e) {
    console.error('Failed to fetch workflows', e)
  } finally {
    loadingWorkflows.value = false
  }
}

// --- Lines ---
//
// A line is a chain of workflows run as one thing: photo → clip → interpolate
// → 4K upscale, each stage reading what the one before it made. Picking one
// takes the place of picking a workflow, because the line already says which
// graphs run, in what order, and with what set on each of them.
const lines = ref([])
const selectedLineId = ref(null)
const selectedLine = computed(() => lines.value.find(l => l.id === selectedLineId.value) || null)

async function fetchLines() {
  try {
    const res = await fetch('/api/comfyui/lines')
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    // A line whose stages no longer fit together cannot be run, and offering
    // it here would only move the refusal later.
    lines.value = (await res.json()).items.filter(l => l.valid)
  } catch (e) {
    console.error('Failed to fetch lines', e)
    lines.value = []
  }
}

function selectLine(id) {
  selectedLineId.value = selectedLineId.value === id ? null : id
  submitError.value = ''
  stageValues.value = {}
}

// --- What the line left open ----------------------------------------------
//
// A stage can pin a setting, sweep it, or leave it to whoever sends the line.
// The third is what this is: the line published its craft and left its subject
// open, so those are the only fields asked for here — and the only ones the
// backend will accept.
const stageValues = ref({})

/** The stages of the picked line that ask for anything, with their controls. */
const askedStages = computed(() => {
  const line = selectedLine.value
  if (!line) return []
  return line.stages
    .filter((stage) => (stage.exposed || []).length)
    .map((stage) => {
      const workflow = workflows.value.find((w) => w.id === stage.workflow_id)
      const keys = new Set(stage.exposed)
      return {
        key: String(stage.stage_idx),
        stage_idx: stage.stage_idx,
        name: stage.workflow_name,
        inputs: (workflow?.inputs || []).filter((i) => keys.has(inputKey(i))),
        loaderNodeIds: (workflow?.loaders || []).map((l) => l.node_id),
      }
    })
    .filter((s) => s.inputs.length)
})

/** One stage's two override maps, created the first time it is written to. */
function valuesFor(key) {
  if (!stageValues.value[key]) {
    stageValues.value = { ...stageValues.value, [key]: { text_overrides: {}, parameters: {} } }
  }
  return stageValues.value[key]
}

/** Only the stages somebody actually answered for. */
const answeredStages = computed(() =>
  Object.fromEntries(
    Object.entries(stageValues.value).filter(
      ([, v]) =>
        Object.keys(v.text_overrides || {}).length || Object.keys(v.parameters || {}).length,
    ),
  ),
)

async function fetchPresets(workflowId) {
  try {
    const res = await fetch(`/api/comfyui/workflows/${workflowId}/presets`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    presets.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch presets', e)
    presets.value = []
  }
}

async function fetchGenerations(shotId) {
  try {
    const res = await fetch(`/api/comfyui/generations/${shotId}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    generations.value = await res.json()
  } catch (e) {
    console.error('Failed to fetch generations', e)
    generations.value = []
  }
}

// Check if a workflow has any existing generation for this shot
function workflowHasGeneration(workflowId) {
  return generations.value.some(g => g.workflow_id === workflowId)
}

// Check if a preset's overrides match any existing generation for the selected workflow
function presetHasGeneration(preset) {
  return generations.value.some(g => {
    if (g.workflow_id !== selectedWorkflowId.value) return false
    return overridesMatch(g.text_overrides, preset.text_overrides)
  })
}

// Slots this workflow gives Phos no way to choose between — two untitled
// LoadImage nodes, say. The backend binds the first and leaves the rest alone;
// without this the user's only clue would be a clip that does not move.
const bindingWarnings = computed(() => selectedWorkflow.value?.warnings || [])

// Check if current text overrides match any existing generation
const currentMatchesGeneration = computed(() => {
  if (!selectedWorkflowId.value) return false
  return generations.value.some(g => {
    if (g.workflow_id !== selectedWorkflowId.value) return false
    return overridesMatch(g.text_overrides, textOverrides.value)
  })
})

function overridesMatch(a, b) {
  const keysA = Object.keys(a || {})
  const keysB = Object.keys(b || {})
  const allKeys = new Set([...keysA, ...keysB])
  for (const key of allKeys) {
    const valA = (a || {})[key] || ''
    const valB = (b || {})[key] || ''
    if (valA !== valB) return false
  }
  return true
}

// A loader node is fed by the source picker, not by a text box.
function isLoaderInput(wf, input) {
  if (input.node_type === 'LoadImage') return true
  return (wf?.loaders || []).some(l => l.node_id === input.node_id)
}

function defaultOverrides(wf) {
  const overrides = {}
  for (const input of wf?.inputs || []) {
    // A loader's slot is filled by the source file, and a number or a dropdown
    // is not a prompt box. Neither belongs in the override map.
    if (isLoaderInput(wf, input)) continue
    if (!isTextInput(input)) continue
    overrides[inputKey(input)] = String(input.current_value ?? '')
  }
  return overrides
}

/** Every typed field, starting at whatever the workflow's author saved. */
function defaultParameters(wf) {
  const params = {}
  for (const input of wf?.inputs || []) {
    if (isLoaderInput(wf, input)) continue
    if (!isParameterInput(input)) continue
    params[inputKey(input)] = parameterValue(input)
  }
  return params
}

/** The node ids the source picker fills, so the control list can skip them. */
const loaderNodeIds = computed(() =>
  (selectedWorkflow.value?.loaders || []).map(l => l.node_id),
)

// Initialize overrides when workflow changes
watch(selectedWorkflow, (wf) => {
  if (!wf) {
    textOverrides.value = {}
    parameters.value = {}
    vary.value = {}
    presets.value = []
    selectedPresetId.value = null
    return
  }
  textOverrides.value = defaultOverrides(wf)
  parameters.value = defaultParameters(wf)
  vary.value = {}
  selectedPresetId.value = null
  // Follow the workflow's own default until the user says otherwise.
  if (!sourceModeTouched.value) sourceModeKey.value = defaultSourceModeKey()
  fetchPresets(wf.id)
})

// Fetch workflows and generations when dialog opens
watch(dialogOpen, (val) => {
  if (val) {
    submitError.value = ''
    submitSuccess.value = false
    sourceModeTouched.value = false
    sourceModeKey.value = defaultSourceModeKey()
    selectedLineId.value = null
    fetchWorkflows()
    fetchLines()
    if (props.shotId) {
      fetchGenerations(props.shotId)
    }
  }
})

function selectWorkflow(id) {
  selectedWorkflowId.value = id
}

function selectPreset(preset) {
  if (selectedPresetId.value === preset.id) {
    // Deselect — restore workflow defaults
    selectedPresetId.value = null
    textOverrides.value = defaultOverrides(selectedWorkflow.value)
    parameters.value = defaultParameters(selectedWorkflow.value)
    return
  }
  selectedPresetId.value = preset.id
  textOverrides.value = { ...textOverrides.value, ...(preset.text_overrides || {}) }
  // A preset saved before parameters existed has none, and leaves the
  // workflow's own values in place rather than blanking them.
  parameters.value = { ...parameters.value, ...(preset.parameters || {}) }
}

const outputType = computed(() => {
  if (!selectedWorkflow.value?.outputs?.length) return null
  return selectedWorkflow.value.outputs[0].node_type || 'image'
})

/** How many tasks pressing Enhance queues. */
const runs = computed(() => runCount(vary.value))
const tooManyRuns = computed(() => runs.value > MAX_FANOUT)

/**
 * Start the picked line against this shot.
 *
 * The stages after the first are queued by the worker as each one lands, so
 * this queues one task and answers with the run the board will show.
 */
async function startLineRun() {
  const res = await fetch('/api/comfyui/runs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      line_id: selectedLineId.value,
      shot_id: props.shotId,
      ...(Object.keys(answeredStages.value).length
        ? { stage_values: answeredStages.value }
        : {}),
    }),
  })
  if (!res.ok) {
    const data = await res.json().catch(() => ({}))
    throw new Error(data.error || `HTTP ${res.status}`)
  }
  return res.json()
}

async function enhance() {
  if (!props.shotId) return
  if (!selectedLineId.value && !selectedWorkflowId.value) return
  submitting.value = true
  submitError.value = ''
  submitSuccess.value = false

  if (selectedLineId.value) {
    try {
      const run = await startLineRun()
      submitSuccess.value = true
      emit('taskCreated', run)
      setTimeout(() => { dialogOpen.value = false }, 800)
    } catch (e) {
      console.error('Run failed to start', e)
      submitError.value = e.message || 'Failed to start the line'
    } finally {
      submitting.value = false
    }
    return
  }

  try {
    const res = await fetch('/api/comfyui/enhance', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        shot_id: props.shotId,
        workflow_id: selectedWorkflowId.value,
        text_overrides: textOverrides.value,
        parameters: parameters.value,
        ...(Object.keys(vary.value).length ? { vary: vary.value } : {}),
        ...(props.fileId ? { source_file_id: props.fileId } : {}),
        ...(sourceMode.value ? { source_mode: sourceMode.value } : {}),
      }),
    })
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      throw new Error(data.error || `HTTP ${res.status}`)
    }
    const task = await res.json()
    submitSuccess.value = true
    emit('taskCreated', task)
    setTimeout(() => {
      dialogOpen.value = false
    }, 800)
  } catch (e) {
    console.error('Enhance failed', e)
    submitError.value = e.message || 'Failed to start enhancement'
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div
    v-if="dialogOpen"
    class="fixed inset-0 z-50 flex items-center justify-center p-4"
    style="background: var(--scrim)"
    @click="dialogOpen = false"
  >
    <div
      class="w-[480px] max-w-full max-h-[calc(100vh-64px)] bg-overlay border border-line-strong rounded shadow-lg flex flex-col overflow-hidden"
      @click.stop
    >
      <div class="flex items-center justify-between px-6 py-4 border-b border-line">
        <div class="font-heading text-base font-semibold text-ink">
          Enhance <span class="font-mono font-medium text-ink-secondary">{{ shotLabel }}</span>
        </div>
        <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="dialogOpen = false">✕</button>
      </div>

      <div class="p-6 flex flex-col gap-6 overflow-y-auto overflow-x-hidden min-h-0">
        <div v-if="loadingWorkflows" class="font-mono text-xs text-ink-tertiary">loading workflows…</div>

        <div v-else-if="workflows.length === 0" class="flex flex-col gap-2">
          <div class="font-mono text-xs text-ink-tertiary">no workflows imported</div>
          <div class="text-xs font-light text-ink-secondary">
            Import a ComfyUI workflow first — Workflows › Import workflow.
          </div>
        </div>

        <template v-else>
          <!-- Line. Picking one replaces picking a workflow: the chain already
               says which graphs run and in what order. -->
          <div v-if="lines.length" class="flex flex-col gap-2">
            <div class="label">Line</div>
            <div class="flex flex-wrap gap-2">
              <button
                v-for="ln in lines"
                :key="ln.id"
                class="flex items-center gap-1.5 whitespace-nowrap border rounded px-3 py-1.5 font-mono text-xs transition-colors"
                :class="selectedLineId === ln.id
                  ? 'border-signal bg-surface text-signal'
                  : 'border-line text-ink-secondary hover:bg-raised'"
                @click="selectLine(ln.id)"
              >
                {{ ln.name }}
                <span class="text-[10px] tracking-[0.08em] uppercase text-ink-tertiary">
                  {{ ln.stage_count }} st
                </span>
              </button>
            </div>
            <div v-if="selectedLine" class="font-mono text-[11px] text-ink-tertiary">
              <template v-for="(st, i) in selectedLine.stages" :key="st.stage_idx">
                <span v-if="i">&nbsp;→&nbsp;</span>{{ st.workflow_name }}
              </template>
            </div>
          </div>

          <!-- What the line left open. Everything else it already decided, and
               a value sent for one of those is refused by name. -->
          <div v-if="askedStages.length" class="flex flex-col gap-2">
            <div class="label">This line asks for</div>
            <div v-for="asked in askedStages" :key="asked.key" class="flex flex-col gap-1.5">
              <div class="flex items-baseline gap-2">
                <span class="label">St {{ asked.stage_idx + 1 }}</span>
                <span class="font-mono text-[11px] text-ink-secondary truncate">{{ asked.name }}</span>
              </div>
              <WorkflowInputControls
                v-model:text-overrides="valuesFor(asked.key).text_overrides"
                v-model:parameters="valuesFor(asked.key).parameters"
                :inputs="asked.inputs"
                :loader-node-ids="asked.loaderNodeIds"
              />
            </div>
          </div>

          <!-- Workflow -->
          <div v-if="!selectedLineId" class="flex flex-col gap-2">
            <div class="label">Workflow</div>
            <div class="flex flex-wrap gap-2">
              <button
                v-for="wf in workflows"
                :key="wf.id"
                class="flex items-center gap-1.5 whitespace-nowrap border rounded px-3 py-1.5 font-mono text-xs transition-colors"
                :class="selectedWorkflowId === wf.id
                  ? 'border-signal bg-surface text-signal'
                  : 'border-line text-ink-secondary hover:bg-raised'"
                @click="selectWorkflow(wf.id)"
              >
                {{ wf.name }}
                <span
                  v-if="workflowHasGeneration(wf.id)"
                  title="This shot already has a variation from this workflow"
                  class="w-1.5 h-1.5 rounded-full"
                  style="background: var(--status-ready)"
                ></span>
              </button>
            </div>
          </div>

          <!-- Source (videos only — a still has no frames to choose between) -->
          <div v-if="sourceIsVideo && !selectedLineId" class="flex flex-col gap-2">
            <div class="label">Source</div>
            <div class="flex flex-wrap gap-2">
              <button
                v-for="mode in SOURCE_MODES"
                :key="mode.key"
                :title="mode.note"
                class="whitespace-nowrap border rounded px-3 py-1.5 font-mono text-xs transition-colors"
                :class="sourceModeKey === mode.key
                  ? 'border-signal bg-surface text-signal'
                  : 'border-line text-ink-secondary hover:bg-raised'"
                @click="selectSourceMode(mode.key)"
              >{{ mode.label }}</button>
            </div>
            <div v-if="sourceModeKey === 'at_time'" class="flex items-center gap-2">
              <span class="font-mono text-[11px] text-ink-tertiary">ms into the clip</span>
              <input
                v-model.number="sourceAtMs"
                type="number"
                min="0"
                step="100"
                class="w-32 bg-base border border-line rounded-sm px-3 py-1.5 font-mono text-xs text-ink"
              />
            </div>
            <div v-else-if="sourceModeKey === 'keyframe'" class="flex items-center gap-2">
              <span class="font-mono text-[11px] text-ink-tertiary">keyframe index, from 0</span>
              <input
                v-model.number="sourceKeyframe"
                type="number"
                min="0"
                step="1"
                class="w-32 bg-base border border-line rounded-sm px-3 py-1.5 font-mono text-xs text-ink"
              />
            </div>
            <div
              v-else-if="sourceModeKey === 'whole_video' && selectedWorkflow && !selectedWorkflow.takes_video"
              class="flex items-center gap-2 px-3 py-2 border rounded font-mono text-xs"
              style="border-color: var(--status-degraded); color: var(--status-degraded)"
            >
              <span class="signal-dot" style="width:6px;height:6px;background:var(--status-degraded)"></span>
              this workflow has no video loader to read the clip
            </div>
          </div>

          <!-- Preset -->
          <div v-if="presets.length && !selectedLineId" class="flex flex-col gap-2">
            <div class="label">Preset</div>
            <div class="flex flex-wrap gap-2">
              <button
                v-for="preset in presets"
                :key="preset.id"
                class="whitespace-nowrap border rounded px-3 py-1.5 text-xs transition-colors"
                :class="selectedPresetId === preset.id
                  ? 'border-signal bg-surface text-signal'
                  : 'border-line text-ink-secondary hover:bg-raised'"
                @click="selectPreset(preset)"
              >
                {{ preset.name }}
                <span
                  v-if="presetHasGeneration(preset)"
                  class="ml-1 w-1.5 h-1.5 rounded-full inline-block align-middle"
                  style="background: var(--status-ready)"
                ></span>
              </button>
            </div>
          </div>

          <!-- Input overrides -->
          <div v-if="selectedWorkflow && !selectedLineId" class="flex flex-col gap-2">
            <div class="flex items-baseline gap-2">
              <div class="label">Inputs</div>
              <span class="flex-1"></span>
              <span
                v-if="runs > 1"
                class="font-mono text-[10px] uppercase tracking-[0.08em]"
                :class="tooManyRuns ? '' : 'text-signal'"
                :style="tooManyRuns ? 'color: var(--status-error)' : ''"
              >{{ runs }} runs</span>
            </div>
            <div
              v-if="tooManyRuns"
              class="flex items-center gap-2 px-3 py-2 border rounded font-mono text-xs"
              style="border-color: var(--status-error); color: var(--status-error)"
            >
              <span class="signal-dot" style="width:6px;height:6px;background:var(--status-error)"></span>
              {{ runs }} runs is more than the {{ MAX_FANOUT }} one request may queue
            </div>
            <WorkflowInputControls
              :key="selectedWorkflowId"
              v-model:text-overrides="textOverrides"
              v-model:parameters="parameters"
              v-model:vary="vary"
              :inputs="selectedWorkflow.inputs || []"
              :loader-node-ids="loaderNodeIds"
              allow-vary
              @dirty="selectedPresetId = null"
            />
          </div>

          <!-- Slots more than one loader claims, with nothing in the graph to
               tell them apart. The run still goes ahead with the first, so this
               is the only place a person finds out there was a choice. -->
          <div
            v-for="(warning, i) in (selectedLineId ? [] : bindingWarnings)"
            :key="i"
            class="flex items-start gap-2 px-3 py-2 border rounded font-mono text-xs"
            style="border-color: var(--status-degraded); color: var(--status-degraded)"
          >
            <span class="signal-dot mt-1 flex-none" style="width:6px;height:6px;background:var(--status-degraded)"></span>
            <span>{{ warning }}</span>
          </div>

          <div
            v-if="currentMatchesGeneration && !selectedLineId"
            class="flex items-center gap-2 px-3 py-2 border rounded font-mono text-xs"
            style="border-color: var(--status-degraded); color: var(--status-degraded)"
          >
            <span class="signal-dot" style="width:6px;height:6px;background:var(--status-degraded)"></span>
            shot already has a variation from this workflow
          </div>

          <div v-if="submitError" class="font-mono text-xs text-error">{{ submitError }}</div>
        </template>
      </div>

      <div class="border-t border-line px-6 py-4 flex items-center gap-4 flex-none">
        <button
          class="bg-signal text-signal-fg rounded px-6 py-2 text-[13px] font-medium whitespace-nowrap hover:bg-signal-hover transition-colors disabled:opacity-50"
          :title="tooManyRuns ? `${runs} runs is more than the ${MAX_FANOUT} one request may queue` : ''"
          :disabled="submitting || (!selectedLineId && !selectedWorkflowId) || (!selectedLineId && tooManyRuns)"
          @click="enhance"
        >{{ submitting ? 'Queuing…' : selectedLineId ? 'Run line' : (runs > 1 ? `Enhance ×${runs}` : 'Enhance') }}</button>
        <span v-if="submitSuccess" class="font-mono text-xs text-ready">
          {{ selectedLineId ? 'run started' : runs > 1 ? `${runs} tasks queued` : 'task queued' }} — see Workflows › Queue
        </span>
        <span class="flex-1"></span>
        <span class="font-mono text-[11px] text-ink-tertiary whitespace-nowrap">
          <template v-if="selectedLine">
            {{ selectedLine.stage_count }}-stage line · intermediates are swept when it lands
          </template>
          <template v-else>
            output attaches as a new file<template v-if="outputType"> · {{ outputType }}</template>
          </template>
        </span>
      </div>
    </div>
  </div>
</template>

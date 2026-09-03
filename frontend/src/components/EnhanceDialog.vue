<script setup>
import { ref, computed, watch } from 'vue'
import WorkflowInputControls from '@/components/WorkflowInputControls.vue'
import {
  isTextInput, inputKey, runCount, MAX_FANOUT,
  isDescribeWorkflow, applyCompiledPrompt, slotKey,
} from '@/lib/utils'

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
//
// `parameters` holds only what was actually set — by hand or by a preset.
// An untouched field is *absent*, so the graph's own value runs verbatim: a
// workflow can carry a seed above 2^53 that JSON.parse already rounded here,
// and echoing every field back would overwrite the exact one with the rounded
// one. The controls fall back to displaying the workflow's value themselves.
const textOverrides = ref({})
const parameters = ref({})
const vary = ref({})
/** What is wrong with a row, if anything — set by the controls, gates Enhance. */
const inputProblem = ref('')

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

// --- Description ---
//
// Phos already knows what is in this photograph: a caption, faces clustered to
// named people, the EXIF time and place. The prompt is compiled from that
// rather than retyped for every shot. A describe workflow — one whose contract
// says it hands on text — runs first, takes seconds, and what it says is shown
// here to be corrected before the costly stage is queued. A prompt you cannot
// see or correct is worse than one you typed.
const intent = ref('')
const stylePreset = ref('')
const doNot = ref('')

/** none | running | ready | failed */
const describeState = ref('none')
const describeCached = ref(false)
const describeError = ref('')
const describeFacts = ref(null)
/** The compiled prompt, editable. What is in these boxes is what gets queued. */
const compiled = ref(null)
const promptApplied = ref(false)
/**
 * The negative slot's value before any compiled prompt touched it. Applying
 * merges against this, never against the last application's result, so a
 * constraint deleted from the compiled box stays deleted on the next press.
 */
const negativeBaseline = ref(undefined)
let describePoll = null
let recompileTimer = null

/** Is there a workflow that can describe a photograph at all? */
const describeWorkflow = computed(() => workflows.value.find(isDescribeWorkflow) || null)

/** The stage the compiled prompt would be written into. */
const promptTarget = computed(() =>
  selectedWorkflow.value && !isDescribeWorkflow(selectedWorkflow.value)
    ? selectedWorkflow.value
    : null,
)

function stopDescribePoll() {
  if (describePoll) {
    clearTimeout(describePoll)
    describePoll = null
  }
  if (recompileTimer) {
    clearTimeout(recompileTimer)
    recompileTimer = null
  }
}

function describeParams() {
  const params = new URLSearchParams()
  if (intent.value.trim()) params.set('intent', intent.value.trim())
  if (stylePreset.value.trim()) params.set('style', stylePreset.value.trim())
  if (doNot.value.trim()) params.set('do_not', doNot.value.trim())
  return params
}

function readDescription(data) {
  describeState.value = data.state
  describeCached.value = Boolean(data.cached)
  describeFacts.value = data.facts || null
  describeError.value = data.error || ''
  if (data.state === 'ready' && data.prompt) {
    compiled.value = { positive: data.prompt.positive || '', negative: data.prompt.negative || '' }
  }
}

/** Ask for a description, or read the one this shot already carries. */
async function describe(refresh = false) {
  if (!props.shotId) return
  stopDescribePoll()
  describeError.value = ''
  promptApplied.value = false
  describeState.value = 'running'
  try {
    const res = await fetch('/api/comfyui/describe', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        shot_id: props.shotId,
        ...(describeWorkflow.value ? { workflow_id: describeWorkflow.value.id } : {}),
        ...(intent.value.trim() ? { intent: intent.value.trim() } : {}),
        ...(stylePreset.value.trim() ? { style: stylePreset.value.trim() } : {}),
        ...(doNot.value.trim()
          ? { do_not: doNot.value.split(/[\n;]/).map(s => s.trim()).filter(Boolean) }
          : {}),
        ...(refresh ? { refresh: true } : {}),
      }),
    })
    const data = await res.json().catch(() => ({}))
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)
    readDescription(data)
    if (data.state === 'running') pollDescription()
  } catch (e) {
    describeState.value = 'failed'
    describeError.value = e.message || 'Could not describe this shot'
  }
}

/** Wait for the describe run. It is seconds, not minutes, so this is short. */
function pollDescription(attempt = 0) {
  stopDescribePoll()
  if (attempt > 90) {
    describeState.value = 'failed'
    describeError.value = 'the describe run is taking longer than expected — see Workflows › Queue'
    return
  }
  describePoll = setTimeout(async () => {
    try {
      const res = await fetch(`/api/comfyui/describe/${props.shotId}?${describeParams()}`)
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const data = await res.json()
      readDescription(data)
      if (data.state === 'running' || data.state === 'none') pollDescription(attempt + 1)
    } catch (e) {
      describeState.value = 'failed'
      describeError.value = e.message || 'Lost track of the describe run'
    }
  }, 2000)
}

/** Write what is in the boxes into the workflow's prompt slots. */
function useCompiledPrompt() {
  if (!promptTarget.value || !compiled.value) return
  const negKey = slotKey(promptTarget.value, 'negative')
  if (negKey && negativeBaseline.value === undefined) {
    negativeBaseline.value = textOverrides.value[negKey] ?? ''
  }
  textOverrides.value = applyCompiledPrompt(
    promptTarget.value,
    textOverrides.value,
    compiled.value,
    negativeBaseline.value,
  )
  selectedPresetId.value = null
  promptApplied.value = true
}

// A prompt compiled for one intent is stale the moment the intent is retyped,
// and "Use this prompt" must never quietly queue the old words. Compiling is a
// pure function of the description — the cheap GET — so the boxes follow the
// typing without describing the photograph again.
watch([intent, stylePreset, doNot], () => {
  if (describeState.value !== 'ready' || !props.shotId) return
  if (recompileTimer) clearTimeout(recompileTimer)
  recompileTimer = setTimeout(async () => {
    try {
      const res = await fetch(`/api/comfyui/describe/${props.shotId}?${describeParams()}`)
      if (!res.ok) return
      const data = await res.json()
      if (data.state === 'ready') readDescription(data)
    } catch {
      // Keep the prompt we have; the next edit or describe will try again.
    }
  }, 400)
})

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
  // An asked key may still carry the value it had back when it was pinned, and
  // the worker falls back to that value when no answer arrives. Start the
  // controls from it, so what the dialog shows is what an untouched field runs
  // — the workflow's own default here would be a value the run never uses.
  const seeded = {}
  for (const stage of selectedLine.value?.stages || []) {
    for (const key of stage.exposed || []) {
      const params = stage.parameters || {}
      const texts = stage.text_overrides || {}
      if (!(key in params) && !(key in texts)) continue
      const slot = (seeded[String(stage.stage_idx)] ||= { text_overrides: {}, parameters: {} })
      if (key in params) slot.parameters[key] = params[key]
      else slot.text_overrides[key] = texts[key]
    }
  }
  stageValues.value = seeded
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

// Check if a preset's overrides and parameters match any existing generation
// for the selected workflow
function presetHasGeneration(preset) {
  return generations.value.some(g => {
    if (g.workflow_id !== selectedWorkflowId.value) return false
    return overridesMatch(g.text_overrides, preset.text_overrides)
      && parametersMatch(g.parameters, preset.parameters)
  })
}

// Slots this workflow gives Phos no way to choose between — two untitled
// LoadImage nodes, say. The backend binds the first and leaves the rest alone;
// without this the user's only clue would be a clip that does not move.
const bindingWarnings = computed(() => selectedWorkflow.value?.warnings || [])

// Check if the current overrides and parameters match any existing generation
const currentMatchesGeneration = computed(() => {
  if (!selectedWorkflowId.value) return false
  return generations.value.some(g => {
    if (g.workflow_id !== selectedWorkflowId.value) return false
    return overridesMatch(g.text_overrides, textOverrides.value)
      && parametersMatch(g.parameters, parameters.value)
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

// Typed values compare as JSON, so 6.5 matches 6.5 and "a.safetensors" only
// itself. Both sides hold only what a run actually set, so a changed seed is
// a different setup even when the prompt is the same.
function parametersMatch(a, b) {
  const allKeys = new Set([...Object.keys(a || {}), ...Object.keys(b || {})])
  for (const key of allKeys) {
    if (JSON.stringify((a || {})[key]) !== JSON.stringify((b || {})[key])) return false
  }
  return true
}

// The one field the source picker fills on a loader node. Its other fields —
// a video loader's frame limits, say — keep their controls.
function isLoaderInput(wf, input) {
  if (input.node_type === 'LoadImage') return true
  return (wf?.loaders || []).some(
    l => l.node_id === input.node_id && l.field === input.field_name,
  )
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

/** The loader fields the source picker fills, so the control list can skip them. */
const loaderKeys = computed(() =>
  (selectedWorkflow.value?.loaders || []).map(l => `${l.node_id}.${l.field}`),
)

// Initialize overrides when workflow changes
watch(selectedWorkflow, (wf) => {
  if (!wf) {
    textOverrides.value = {}
    parameters.value = {}
    vary.value = {}
    presets.value = []
    selectedPresetId.value = null
    negativeBaseline.value = undefined
    return
  }
  textOverrides.value = defaultOverrides(wf)
  parameters.value = {}
  vary.value = {}
  selectedPresetId.value = null
  negativeBaseline.value = undefined
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
    describeState.value = 'none'
    describeCached.value = false
    describeError.value = ''
    describeFacts.value = null
    compiled.value = null
    promptApplied.value = false
    negativeBaseline.value = undefined
    fetchWorkflows()
    fetchLines()
    if (props.shotId) {
      fetchGenerations(props.shotId)
      // Free: the description this shot already carries, and the prompt it
      // compiles to. Nothing is described and no GPU is asked for anything
      // until the button is pressed.
      fetch(`/api/comfyui/describe/${props.shotId}?${describeParams()}`)
        .then(r => (r.ok ? r.json() : null))
        .then(d => {
          if (!d) return
          readDescription(d)
          if (d.state === 'running') pollDescription()
        })
        .catch(() => {})
    }
  } else {
    stopDescribePoll()
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
    parameters.value = {}
    negativeBaseline.value = undefined
    return
  }
  // A preset lands on the workflow's own values, never on whatever the last
  // preset left behind — a prompt-only preset saved before parameters existed
  // must run the workflow's seed and model, not its predecessor's.
  selectedPresetId.value = preset.id
  textOverrides.value = { ...defaultOverrides(selectedWorkflow.value), ...(preset.text_overrides || {}) }
  parameters.value = { ...(preset.parameters || {}) }
  negativeBaseline.value = undefined
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

          <!-- Description.
               Phos knows what is in this photograph — the caption, the people
               clustering named, the EXIF time and place. The prompt is compiled
               from that rather than retyped, and it is shown here so it can be
               corrected before the costly stage is queued. -->
          <div v-if="describeWorkflow && !selectedLineId" class="flex flex-col gap-3">
            <div class="flex items-baseline gap-2">
              <div class="label">Description</div>
              <span class="flex-1"></span>
              <span
                v-if="describeCached && describeState === 'ready'"
                class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary"
              >stored for this shot</span>
            </div>

            <div class="flex flex-col gap-2">
              <input
                v-model="intent"
                type="text"
                placeholder="what you are after — a slow push-in as the light fades"
                class="w-full bg-base border border-line rounded-sm px-3 py-1.5 text-xs text-ink"
              />
              <input
                v-model="stylePreset"
                type="text"
                placeholder="style — 35mm film, muted palette"
                class="w-full bg-base border border-line rounded-sm px-3 py-1.5 text-xs text-ink"
              />
              <textarea
                v-model="doNot"
                rows="2"
                placeholder="must not — one per line: change face"
                class="w-full bg-base border border-line rounded-sm px-3 py-1.5 text-xs text-ink resize-y"
              ></textarea>
            </div>

            <div class="flex items-center gap-3">
              <button
                class="border border-line rounded px-3 py-1.5 font-mono text-xs text-ink-secondary hover:bg-raised transition-colors disabled:opacity-50"
                :disabled="describeState === 'running'"
                @click="describe(describeState === 'ready')"
              >{{ describeState === 'ready' ? 'Describe again' : 'Describe' }}</button>
              <span
                v-if="describeState === 'running'"
                class="flex items-center gap-2 font-mono text-[11px] text-ink-tertiary"
              >
                <span class="signal-dot signal-pulse" style="width:6px;height:6px;background:var(--status-pending)"></span>
                reading the photograph…
              </span>
              <span
                v-else-if="describeFacts && (describeFacts.people || []).length"
                class="font-mono text-[11px] text-ink-tertiary truncate"
              >knows: {{ (describeFacts.people || []).join(', ') }}<template v-if="describeFacts.taken_at"> · {{ describeFacts.taken_at.slice(0, 10) }}</template></span>
            </div>

            <div
              v-if="describeState === 'failed'"
              class="flex items-start gap-2 px-3 py-2 border rounded font-mono text-xs"
              style="border-color: var(--status-error); color: var(--status-error)"
            >
              <span class="signal-dot mt-1 flex-none" style="width:6px;height:6px;background:var(--status-error)"></span>
              <span>{{ describeError }}</span>
            </div>

            <!-- Editable, and deliberately so: this is what gets queued. -->
            <div v-if="compiled && describeState === 'ready'" class="flex flex-col gap-2">
              <div class="label">Compiled prompt</div>
              <textarea
                v-model="compiled.positive"
                rows="5"
                class="w-full bg-base border border-line rounded-sm px-3 py-2 text-xs text-ink resize-y leading-relaxed"
              ></textarea>
              <div class="label">Must not</div>
              <textarea
                v-model="compiled.negative"
                rows="2"
                class="w-full bg-base border border-line rounded-sm px-3 py-2 font-mono text-[11px] text-ink resize-y"
              ></textarea>
              <div class="flex items-center gap-3">
                <button
                  class="border rounded px-3 py-1.5 font-mono text-xs transition-colors disabled:opacity-50"
                  :class="promptApplied ? 'border-line text-ink-tertiary' : 'border-signal text-signal hover:bg-surface'"
                  :disabled="!promptTarget"
                  @click="useCompiledPrompt"
                >{{ promptApplied ? 'in the prompt boxes' : 'Use this prompt' }}</button>
                <span v-if="!promptTarget" class="font-mono text-[11px] text-ink-tertiary">
                  pick the workflow this prompt is for
                </span>
                <span v-else-if="!slotKey(promptTarget, 'positive')" class="font-mono text-[11px]" style="color: var(--status-degraded)">
                  {{ promptTarget.name }} has no prompt box to write into
                </span>
              </div>
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
              v-model:problem="inputProblem"
              :inputs="selectedWorkflow.inputs || []"
              :loader-keys="loaderKeys"
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
          :title="tooManyRuns ? `${runs} runs is more than the ${MAX_FANOUT} one request may queue` : inputProblem"
          :disabled="submitting || (!selectedLineId && (!selectedWorkflowId || tooManyRuns || !!inputProblem))"
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

<script setup>
import { ref, computed, watch } from 'vue'
import WorkflowInputControls from '@/components/WorkflowInputControls.vue'
import { isTextInput, inputKey, runCount, MAX_FANOUT } from '@/lib/utils'

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
    return
  }
  textOverrides.value = defaultOverrides(wf)
  parameters.value = {}
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
    fetchWorkflows()
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
    parameters.value = {}
    return
  }
  // A preset lands on the workflow's own values, never on whatever the last
  // preset left behind — a prompt-only preset saved before parameters existed
  // must run the workflow's seed and model, not its predecessor's.
  selectedPresetId.value = preset.id
  textOverrides.value = { ...defaultOverrides(selectedWorkflow.value), ...(preset.text_overrides || {}) }
  parameters.value = { ...(preset.parameters || {}) }
}

const outputType = computed(() => {
  if (!selectedWorkflow.value?.outputs?.length) return null
  return selectedWorkflow.value.outputs[0].node_type || 'image'
})

/** How many tasks pressing Enhance queues. */
const runs = computed(() => runCount(vary.value))
const tooManyRuns = computed(() => runs.value > MAX_FANOUT)

async function enhance() {
  if (!selectedWorkflowId.value || !props.shotId) return
  submitting.value = true
  submitError.value = ''
  submitSuccess.value = false

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
          <!-- Workflow -->
          <div class="flex flex-col gap-2">
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
          <div v-if="sourceIsVideo" class="flex flex-col gap-2">
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
          <div v-if="presets.length" class="flex flex-col gap-2">
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
          <div v-if="selectedWorkflow" class="flex flex-col gap-2">
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

          <div
            v-if="currentMatchesGeneration"
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
          :disabled="submitting || !selectedWorkflowId || tooManyRuns || !!inputProblem"
          @click="enhance"
        >{{ submitting ? 'Queuing…' : (runs > 1 ? `Enhance ×${runs}` : 'Enhance') }}</button>
        <span v-if="submitSuccess" class="font-mono text-xs text-ready">
          {{ runs > 1 ? `${runs} tasks queued` : 'task queued' }} — see Workflows › Queue
        </span>
        <span class="flex-1"></span>
        <span class="font-mono text-[11px] text-ink-tertiary whitespace-nowrap">
          output attaches as a new file<template v-if="outputType"> · {{ outputType }}</template>
        </span>
      </div>
    </div>
  </div>
</template>

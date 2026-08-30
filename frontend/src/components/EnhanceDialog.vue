<script setup>
import { ref, computed, watch } from 'vue'
import { isTextInput } from '@/lib/utils'

const props = defineProps({
  open: Boolean,
  shotId: [String, Number],
  shotLabel: { type: String, default: '' },
  fileId: String,
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

// --- Text overrides ---
const textOverrides = ref({})

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

// Check if a preset's overrides match any existing generation for the selected workflow
function presetHasGeneration(preset) {
  return generations.value.some(g => {
    if (g.workflow_id !== selectedWorkflowId.value) return false
    return overridesMatch(g.text_overrides, preset.text_overrides)
  })
}

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

// Initialize text overrides when workflow changes
watch(selectedWorkflow, (wf) => {
  if (!wf) {
    textOverrides.value = {}
    presets.value = []
    selectedPresetId.value = null
    return
  }
  const overrides = {}
  const inputs = wf.inputs || []
  for (const input of inputs) {
    if (isTextInput(input)) {
      overrides[`${input.node_id}.${input.field_name}`] = String(input.current_value ?? '')
    }
  }
  textOverrides.value = overrides
  selectedPresetId.value = null
  fetchPresets(wf.id)
})

// Fetch workflows and generations when dialog opens
watch(dialogOpen, (val) => {
  if (val) {
    submitError.value = ''
    submitSuccess.value = false
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
    const overrides = {}
    const inputs = selectedWorkflow.value?.inputs || []
    for (const input of inputs) {
      if (isTextInput(input)) {
        overrides[`${input.node_id}.${input.field_name}`] = String(input.current_value ?? '')
      }
    }
    textOverrides.value = overrides
    return
  }
  selectedPresetId.value = preset.id
  const overrides = { ...textOverrides.value }
  for (const [key, value] of Object.entries(preset.text_overrides)) {
    overrides[key] = value
  }
  textOverrides.value = overrides
}

const textInputs = computed(() => {
  if (!selectedWorkflow.value) return []
  return (selectedWorkflow.value.inputs || []).filter(isTextInput)
})

const outputType = computed(() => {
  if (!selectedWorkflow.value?.outputs?.length) return null
  return selectedWorkflow.value.outputs[0].node_type || 'image'
})

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
        ...(props.fileId ? { source_file_id: props.fileId } : {}),
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
          <div v-if="textInputs.length" class="flex flex-col gap-2">
            <div class="label">Input overrides</div>
            <div v-for="input in textInputs" :key="`${input.node_id}.${input.field_name}`" class="flex flex-col gap-1">
              <span class="font-mono text-[11px] text-ink-tertiary">
                {{ input.node_id }} · {{ input.node_type }} · {{ input.field_name }}
              </span>
              <textarea
                v-model="textOverrides[`${input.node_id}.${input.field_name}`]"
                rows="2"
                spellcheck="false"
                class="w-full bg-base border border-line rounded-sm px-3 py-2 font-mono text-xs text-ink"
                @input="selectedPresetId = null"
              ></textarea>
            </div>
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
          class="bg-signal text-signal-fg rounded px-6 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
          :disabled="submitting || !selectedWorkflowId"
          @click="enhance"
        >{{ submitting ? 'Queuing…' : 'Enhance' }}</button>
        <span v-if="submitSuccess" class="font-mono text-xs text-ready">task queued — see Workflows › Queue</span>
        <span class="flex-1"></span>
        <span class="font-mono text-[11px] text-ink-tertiary whitespace-nowrap">
          output attaches as a new file<template v-if="outputType"> · {{ outputType }}</template>
        </span>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Wand2,
  RefreshCw,
  Check,
  AlertCircle,
  Copy,
} from 'lucide-vue-next'

const props = defineProps({
  open: Boolean,
  shotId: [String, Number],
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
    if (input.node_type !== 'LoadImage') {
      overrides[`${input.node_id}.${input.field_name}`] = typeof input.current_value === 'string' ? input.current_value : ''
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
      if (input.node_type !== 'LoadImage') {
        overrides[`${input.node_id}.${input.field_name}`] = typeof input.current_value === 'string' ? input.current_value : ''
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
  return (selectedWorkflow.value.inputs || []).filter(
    i => i.node_type !== 'LoadImage'
  )
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
  <Dialog v-model:open="dialogOpen">
    <DialogContent class="sm:max-w-[500px]">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2">
          <Wand2 class="w-5 h-5 text-indigo-400" />
          Enhance with AI
        </DialogTitle>
        <DialogDescription>
          Select a ComfyUI workflow and customize inputs to enhance this shot.
        </DialogDescription>
      </DialogHeader>

      <div class="mt-4 space-y-4">
        <!-- Workflow selector (chips) -->
        <div class="space-y-2">
          <Label>Workflow</Label>
          <div v-if="loadingWorkflows" class="text-sm text-zinc-500">Loading workflows...</div>
          <div v-else-if="workflows.length" class="flex flex-wrap gap-2">
            <button
              v-for="wf in workflows"
              :key="wf.id"
              class="relative px-3 py-1.5 rounded-lg text-sm font-medium border transition-all"
              :class="selectedWorkflowId === wf.id
                ? 'bg-indigo-600/20 text-indigo-300 border-indigo-500/30'
                : 'bg-zinc-800/50 text-zinc-400 border-white/10 hover:border-white/20 hover:text-zinc-200'"
              @click="selectWorkflow(wf.id)"
            >
              {{ wf.name }}
              <span
                v-if="workflowHasGeneration(wf.id)"
                class="ml-1.5 inline-flex items-center"
                title="This shot already has variations from this workflow"
              >
                <Copy class="w-3 h-3 text-emerald-400" />
              </span>
            </button>
          </div>
          <div v-else class="text-sm text-zinc-500">No workflows available</div>
        </div>

        <!-- Output type indicator -->
        <div v-if="outputType" class="flex items-center gap-2">
          <span class="text-xs text-zinc-500">Output:</span>
          <span class="px-2 py-0.5 rounded-full bg-indigo-500/10 border border-indigo-500/20 text-xs font-medium text-indigo-400">
            {{ outputType }}
          </span>
        </div>

        <!-- Preset chips -->
        <div v-if="presets.length && selectedWorkflow" class="space-y-2">
          <Label class="text-zinc-400">Presets</Label>
          <div class="flex flex-wrap gap-2">
            <button
              v-for="preset in presets"
              :key="preset.id"
              class="relative px-3 py-1.5 rounded-lg text-sm font-medium border transition-all"
              :class="[
                selectedPresetId === preset.id
                  ? 'bg-amber-600/20 text-amber-300 border-amber-500/30'
                  : 'bg-zinc-800/50 text-zinc-400 border-white/10 hover:border-white/20 hover:text-zinc-200',
                presetHasGeneration(preset) ? 'ring-1 ring-emerald-500/30' : '',
              ]"
              @click="selectPreset(preset)"
            >
              {{ preset.name }}
              <span
                v-if="presetHasGeneration(preset)"
                class="ml-1.5 inline-flex items-center"
                title="Already generated with this preset"
              >
                <Check class="w-3 h-3 text-emerald-400" />
              </span>
            </button>
          </div>
        </div>

        <!-- Text input overrides -->
        <div v-if="textInputs.length" class="space-y-3">
          <Label class="text-zinc-400">Input Overrides</Label>
          <div v-for="input in textInputs" :key="`${input.node_id}.${input.field_name}`" class="space-y-1.5">
            <label class="text-xs font-medium text-zinc-400">
              {{ input.field_name }} <span class="text-zinc-600">({{ input.node_type }}, node {{ input.node_id }})</span>
            </label>
            <textarea
              v-model="textOverrides[`${input.node_id}.${input.field_name}`]"
              rows="2"
              class="flex w-full rounded-lg border border-white/10 bg-zinc-800/50 px-3 py-2 text-sm text-zinc-200 placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500/40 focus-visible:ring-offset-0 resize-y"
              :placeholder="typeof input.current_value === 'string' ? input.current_value : 'Enter value...'"
              @input="selectedPresetId = null"
            />
          </div>
        </div>

        <!-- Already generated warning -->
        <div v-if="currentMatchesGeneration" class="flex items-start gap-2 p-3 rounded-xl bg-amber-500/10 border border-amber-500/20">
          <Copy class="w-4 h-4 text-amber-400 mt-0.5 shrink-0" />
          <p class="text-sm text-amber-300">This shot already has a variation with this workflow and prompts.</p>
        </div>

        <!-- No workflows message -->
        <div v-if="!loadingWorkflows && workflows.length === 0" class="text-center py-6">
          <Wand2 class="w-8 h-8 text-zinc-600 mx-auto mb-2" />
          <p class="text-sm text-zinc-400">No workflows available</p>
          <p class="text-xs text-zinc-500 mt-1">Import a workflow in the Workflows page first.</p>
        </div>

        <!-- Feedback -->
        <div v-if="submitError" class="flex items-start gap-2 p-3 rounded-xl bg-red-500/10 border border-red-500/20">
          <AlertCircle class="w-4 h-4 text-red-500 mt-0.5 shrink-0" />
          <p class="text-sm text-red-400">{{ submitError }}</p>
        </div>
        <div v-if="submitSuccess" class="flex items-start gap-2 p-3 rounded-xl bg-emerald-500/10 border border-emerald-500/20">
          <Check class="w-4 h-4 text-emerald-500 mt-0.5 shrink-0" />
          <p class="text-sm text-emerald-400">Enhancement queued successfully!</p>
        </div>

        <!-- Enhance button -->
        <Button
          class="w-full bg-indigo-600 hover:bg-indigo-500 text-white shadow-lg shadow-indigo-500/20 gap-2"
          :disabled="!selectedWorkflowId || submitting || submitSuccess"
          @click="enhance"
        >
          <RefreshCw v-if="submitting" class="w-4 h-4 animate-spin" />
          <Wand2 v-else class="w-4 h-4" />
          {{ submitting ? 'Enhancing...' : 'Enhance' }}
        </Button>
      </div>
    </DialogContent>
  </Dialog>
</template>

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
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Wand2,
  RefreshCw,
  Check,
  AlertCircle,
  ChevronDown,
} from 'lucide-vue-next'

const props = defineProps({
  open: Boolean,
  shotId: [String, Number],
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

// --- Text overrides ---
const textOverrides = ref({})

// --- Submit state ---
const submitting = ref(false)
const submitError = ref('')
const submitSuccess = ref(false)

// --- Dropdown open ---
const showDropdown = ref(false)

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

// Initialize text overrides when workflow changes
watch(selectedWorkflow, (wf) => {
  if (!wf) {
    textOverrides.value = {}
    return
  }
  const overrides = {}
  const inputs = wf.inputs || []
  for (const input of inputs) {
    if (input.node_type !== 'LoadImage') {
      overrides[input.node_id] = typeof input.current_value === 'string' ? input.current_value : ''
    }
  }
  textOverrides.value = overrides
})

// Fetch workflows when dialog opens
watch(dialogOpen, (val) => {
  if (val) {
    submitError.value = ''
    submitSuccess.value = false
    fetchWorkflows()
  }
})

function selectWorkflow(id) {
  selectedWorkflowId.value = id
  showDropdown.value = false
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
        <!-- Workflow selector -->
        <div class="space-y-2">
          <Label>Workflow</Label>
          <div class="relative">
            <button
              class="w-full flex items-center justify-between px-3 py-2 rounded-lg bg-zinc-800/50 border border-white/10 text-sm text-left hover:border-white/20 transition-colors"
              @click="showDropdown = !showDropdown"
            >
              <span v-if="loadingWorkflows" class="text-zinc-500">Loading workflows...</span>
              <span v-else-if="selectedWorkflow" class="text-zinc-200 truncate">{{ selectedWorkflow.name }}</span>
              <span v-else class="text-zinc-500">Select a workflow...</span>
              <ChevronDown class="w-4 h-4 text-zinc-500 shrink-0 ml-2" />
            </button>

            <div
              v-if="showDropdown && workflows.length"
              class="absolute top-full left-0 right-0 mt-1 z-50 bg-zinc-900 border border-white/10 rounded-xl shadow-2xl overflow-hidden"
            >
              <ScrollArea class="max-h-48">
                <div class="p-1">
                  <button
                    v-for="wf in workflows"
                    :key="wf.id"
                    class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left transition-colors"
                    :class="selectedWorkflowId === wf.id
                      ? 'bg-indigo-600/20 text-indigo-300'
                      : 'text-zinc-300 hover:bg-white/5 hover:text-white'"
                    @click="selectWorkflow(wf.id)"
                  >
                    <div class="min-w-0 flex-1">
                      <p class="text-sm font-medium truncate">{{ wf.name }}</p>
                      <p v-if="wf.description" class="text-xs text-zinc-500 truncate">{{ wf.description }}</p>
                    </div>
                    <Check v-if="selectedWorkflowId === wf.id" class="w-4 h-4 text-indigo-400 shrink-0" />
                  </button>
                </div>
              </ScrollArea>
            </div>
          </div>
        </div>

        <!-- Output type indicator -->
        <div v-if="outputType" class="flex items-center gap-2">
          <span class="text-xs text-zinc-500">Output:</span>
          <span class="px-2 py-0.5 rounded-full bg-indigo-500/10 border border-indigo-500/20 text-xs font-medium text-indigo-400">
            {{ outputType }}
          </span>
        </div>

        <!-- Text input overrides -->
        <div v-if="textInputs.length" class="space-y-3">
          <Label class="text-zinc-400">Input Overrides</Label>
          <div v-for="input in textInputs" :key="input.node_id" class="space-y-1.5">
            <label class="text-xs font-medium text-zinc-400">
              {{ input.field_name }} <span class="text-zinc-600">({{ input.node_type }}, node {{ input.node_id }})</span>
            </label>
            <textarea
              v-model="textOverrides[input.node_id]"
              rows="2"
              class="flex w-full rounded-lg border border-white/10 bg-zinc-800/50 px-3 py-2 text-sm text-zinc-200 placeholder:text-zinc-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500/40 focus-visible:ring-offset-0 resize-y"
              :placeholder="typeof input.current_value === 'string' ? input.current_value : 'Enter value...'"
            />
          </div>
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

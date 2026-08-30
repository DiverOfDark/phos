<script setup>
/**
 * A production line — read as a route board, built as a vertical list.
 *
 * Read-only it draws the way `WorkflowContract.vue` draws a contract and
 * `WorkflowGraph.vue` draws a graph, because a line is the same kind of fact:
 * two terminals and the track between them, with what travels along each join
 * written on it. What goes in one end and what comes out the other is the thing
 * that decides whether a line can be run at all, so it is the thing the board
 * says loudest.
 *
 * Under edit it becomes a list, top to bottom. **Not a canvas** — Phos does not
 * draw graphs, ComfyUI does, and a line is linear by construction. What makes a
 * list enough is the picker: `Add stage` and `Swap` only ever offer workflows
 * that fit where they are going, so an invalid line is not something a person
 * can draw and then be told about.
 *
 * That filter is not computed here. The editor sends the line it is holding and
 * the position being filled, and the server runs *its own validator* over the
 * line each candidate would make. A copy of the rule in JavaScript would be the
 * one way this screen could be wrong in a way nobody notices — a picker that
 * disagreed with the dispatcher would offer stages that then fail four hours
 * into a run — and it would have to be kept in step with rules it does not know
 * about, such as a stage that produces no file being transparent to the media
 * flowing past it.
 *
 * The connector between two stages states the handoff. It is clickable only
 * when there is a real choice: a clip going into a graph that can read both a
 * video and a still, or a stage with more than one slot to put the file in. The
 * options are FR2's source modes and FR2's own `role` directive, not a second
 * mechanism invented here.
 */
import { ref, computed, watch, nextTick, onMounted, onUnmounted } from 'vue'
import WorkflowInputControls from '@/components/WorkflowInputControls.vue'
import { controlKind, inputKey } from '@/lib/utils'
import {
  pickerRequest,
  handoffLabel,
  parseSourceMode,
  sourceMode,
  reorder,
  toPayload,
  typeTrack,
  MODE_WORDS,
} from '@/lib/lines'

const props = defineProps({
  /** The line as `GET /api/comfyui/lines/{id}` served it, or null for a new one. */
  line: { type: Object, default: null },
  /** Every workflow, for the per-stage controls. The picker asks the server. */
  workflows: { type: Array, default: () => [] },
  /** Open in edit mode straight away — what New line and Duplicate want. */
  startEditing: { type: Boolean, default: false },
})

const emit = defineEmits(['saved', 'cancelled', 'duplicate', 'export', 'delete'])

// --- Editing state ---------------------------------------------------------
// A working copy, so cancelling costs nothing and the board underneath keeps
// showing what is actually stored until Save says otherwise.

const editing = ref(false)
const draft = ref(null)
const saving = ref(false)
const saveError = ref('')
const openStage = ref(null)
const openConnector = ref(null)

const isNew = computed(() => !props.line?.id)
const locked = computed(() => props.line?.editable === false)
const stages = computed(() => (editing.value ? draft.value?.stages : props.line?.stages) || [])

function blankDraft() {
  return { name: '', description: '', stages: [] }
}

function startEdit() {
  draft.value = props.line
    ? JSON.parse(JSON.stringify({
        name: props.line.name,
        description: props.line.description || '',
        stages: props.line.stages || [],
      }))
    : blankDraft()
  editing.value = true
  saveError.value = ''
  openStage.value = null
  openConnector.value = null
  check()
}

function cancel() {
  editing.value = false
  draft.value = null
  picker.value = null
  emit('cancelled')
}

// --- Validation ------------------------------------------------------------
// Asked of the server on every change, because the rule that decides it lives
// there. A reorder therefore says whether it holds together where it was
// dragged, rather than after Save.

const verdict = ref({ valid: true, error: null })
let checkTimer = null

function check() {
  clearTimeout(checkTimer)
  if (!draft.value?.stages?.length) {
    verdict.value = { valid: false, error: 'A line needs at least one stage.' }
    return
  }
  checkTimer = setTimeout(async () => {
    try {
      const res = await fetch('/api/comfyui/lines/validate', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(toPayload({ ...draft.value, name: draft.value.name || 'draft' })),
      })
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      const answer = await res.json()
      verdict.value = { valid: answer.valid, error: answer.error }
      // The joins come back with the verdict, worked out by the same code that
      // works out a stored line's — so a stage added a moment ago draws its
      // connector the same way one saved a year ago does.
      ;(answer.stages || []).forEach((s, i) => {
        const stage = draft.value?.stages?.[i]
        if (!stage || s.accepts === undefined) return
        stage.accepts = s.accepts
        stage.produces = s.produces
        stage.handoff = s.handoff
      })
    } catch (e) {
      // A check that could not be made is not a refusal. Save asks again, and
      // that answer is the one that counts.
      verdict.value = { valid: true, error: null }
      console.warn('Could not check the line', e)
    }
  }, 250)
}

// --- The picker ------------------------------------------------------------
// Never a list of every workflow: the server marks each one offered or refused
// for this exact slot, and the refused ones are shown greyed with the reason so
// the screen says what it is not offering and why.

const picker = ref(null)

async function openPicker(index, mode) {
  picker.value = { index, mode, loading: true, items: [], offered: 0, refused: 0, showRefused: false }
  try {
    const res = await fetch('/api/comfyui/lines/stage-options', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(pickerRequest(stages.value, index, mode)),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    picker.value = { index, mode, loading: false, showRefused: false, ...data }
  } catch (e) {
    picker.value = { index, mode, loading: false, items: [], error: e.message, showRefused: false }
  }
}

/** The one line the picker prints under itself, saying what it is filtering on. */
const pickerNote = computed(() => {
  const p = picker.value
  if (!p || p.loading) return ''
  if (!p.filter) return 'every workflow fits here'
  return `${p.filter} · ${p.refused} hidden`
})

function chooseStage(option) {
  if (!option.offered) return
  const fresh = {
    workflow_id: option.workflow_id,
    workflow_name: option.name,
    accepts: option.accepts,
    produces: option.produces,
    text_overrides: {},
    parameters: {},
    vary: {},
    exposed: [],
    source_mode: null,
    keep_output: false,
    hold_for_review: false,
  }
  const list = [...draft.value.stages]
  if (picker.value.mode === 'replace') list.splice(picker.value.index, 1, fresh)
  else list.splice(picker.value.index, 0, fresh)
  draft.value.stages = list
  picker.value = null
  check()
}

// --- Rearranging -----------------------------------------------------------

function move(index, delta) {
  draft.value.stages = reorder(draft.value.stages, index, index + delta)
  openStage.value = null
  check()
}

function removeStage(index) {
  draft.value.stages = draft.value.stages.filter((_, i) => i !== index)
  openStage.value = null
  check()
}

// --- The connector ---------------------------------------------------------

/** What a join says, and whether clicking it asks anything. */
function joinOf(index) {
  return stages.value[index]?.handoff || null
}

function joinLabel(index) {
  return handoffLabel(joinOf(index))
}

function setMode(index, key) {
  const stage = draft.value.stages[index]
  const current = parseSourceMode(stage.source_mode)
  stage.source_mode = key === current.key && key !== 'at_time' ? null : sourceMode(key, current)
  check()
}

function setModeMs(index, ms) {
  const stage = draft.value.stages[index]
  stage.source_mode = sourceMode('at_time', { ms })
  check()
}

/** Which slot the incoming file fills — FR2's own bare `role` directive. */
function setRole(index, role) {
  const stage = draft.value.stages[index]
  stage.text_overrides = { ...(stage.text_overrides || {}), role }
  check()
}

function roleOf(index) {
  return stages.value[index]?.text_overrides?.role || null
}

function modeKeyOf(index) {
  return parseSourceMode(draft.value.stages[index]?.source_mode).key
}

// --- Per-stage settings ----------------------------------------------------

function workflowOf(stage) {
  return props.workflows.find((w) => w.id === stage.workflow_id) || null
}

/** The inputs a person can set — the loaders are fed by the line, not by hand. */
function inputsOf(stage) {
  return (workflowOf(stage)?.inputs || []).filter((i) => controlKind(i) !== null)
}

function loaderIdsOf(stage) {
  return (workflowOf(stage)?.loaders || []).map((l) => l.node_id)
}

function askedCountOf(stage) {
  return (stage.exposed || []).length
}

function variedCountOf(stage) {
  return Object.keys(stage.vary || {}).length
}

/** The keys of a stage that are asked for at send time, as readable names. */
function askedNames(stage) {
  const byKey = new Map(inputsOf(stage).map((i) => [inputKey(i), i]))
  return (stage.exposed || []).map((key) => {
    const input = byKey.get(key)
    return input ? `${input.node_title || input.node_type} · ${input.field_name}` : key
  })
}

// --- Saving ----------------------------------------------------------------

async function save() {
  saving.value = true
  saveError.value = ''
  try {
    const body = toPayload(draft.value)
    const res = await fetch(
      isNew.value ? '/api/comfyui/lines' : `/api/comfyui/lines/${props.line.id}`,
      {
        method: isNew.value ? 'POST' : 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      },
    )
    if (!res.ok) {
      const data = await res.json().catch(() => ({}))
      const error = new Error(data.error || `HTTP ${res.status}`)
      error.status = res.status
      throw error
    }
    const saved = await res.json()
    editing.value = false
    draft.value = null
    await nextTick()
    emit('saved', saved)
  } catch (e) {
    // A 409 is the live-run guard. It is not an error to work around: the line
    // is being walked right now, and the way to change it is to fork it.
    saveError.value = e.message || 'Could not save the line'
    saveConflict.value = e.status === 409
  } finally {
    saving.value = false
  }
}

const saveConflict = ref(false)

const canSave = computed(
  () =>
    !!draft.value?.name?.trim() &&
    (draft.value?.stages || []).length > 0 &&
    verdict.value.valid &&
    !saving.value,
)

const track = computed(() => typeTrack(editing.value ? draft.value : props.line))

/**
 * The board's columns: a terminal, then a join and a stage for each stage, then
 * the last run of track and the other terminal.
 *
 * Stages are a fixed width and joins take what is left, so the whole thing
 * fills the card at two stages and scrolls at eight rather than crushing every
 * workflow name down to three letters.
 */
const BOARD_STAGE_W = 156
const BOARD_JOIN_W = 76
const boardColumns = computed(() => {
  const n = stages.value.length
  const middle = Array.from({ length: n }, () => `minmax(${BOARD_JOIN_W}px,1fr) ${BOARD_STAGE_W}px`)
  return {
    gridTemplateColumns: ['76px', ...middle, 'minmax(36px,1fr)', '76px'].join(' '),
    minWidth: `${152 + n * (BOARD_STAGE_W + BOARD_JOIN_W) + 36}px`,
  }
})

// A board wider than the card is not a bug — a six-stage line is a wide thing —
// but a reader has to be told there is more of it, the way the graph is told.
const boardScroller = ref(null)
const boardOverflows = ref(false)

function measureBoard() {
  const el = boardScroller.value
  boardOverflows.value = !!el && el.scrollWidth > el.clientWidth + 1
}

watch([stages, editing], () => nextTick(measureBoard))
onMounted(() => {
  measureBoard()
  window.addEventListener('resize', measureBoard)
})
onUnmounted(() => window.removeEventListener('resize', measureBoard))

// Last, and deliberately: it runs immediately, and `startEdit` reaches for the
// picker and the validator, which are `const`s declared above this point and
// below the top of the file. A watch placed with the state it resets would
// read them before they exist.
watch(
  () => [props.line?.id, props.startEditing],
  () => {
    if (props.startEditing) startEdit()
    else {
      editing.value = false
      draft.value = null
      picker.value = null
    }
  },
  { immediate: true },
)
</script>

<template>
  <div class="flex flex-col gap-3 min-w-0">
    <!-- Header: what this line is, and what can be done to it. -->
    <div class="flex flex-wrap items-baseline gap-x-4 gap-y-2">
      <template v-if="editing">
        <input
          v-model="draft.name"
          placeholder="4K Restore"
          spellcheck="false"
          aria-label="Line name"
          class="min-w-0 flex-1 bg-base border border-line rounded-sm px-3 py-1.5 text-[13px] text-ink"
        />
      </template>
      <template v-else>
        <span class="font-mono text-base font-medium text-ink truncate">{{ line?.name }}</span>
        <span v-if="track" class="tag text-ink-tertiary">{{ track }}</span>
      </template>

      <span class="flex-1"></span>

      <div class="flex items-center gap-4 font-mono text-[11px]">
        <template v-if="editing">
          <span v-if="verdict.valid" class="text-ink-tertiary">holds together</span>
          <span v-else class="text-error truncate max-w-[420px]" :title="verdict.error">{{ verdict.error }}</span>
        </template>
        <template v-else-if="line">
          <span v-if="locked" class="flex items-center gap-1.5 text-ink-tertiary">
            <span class="signal-dot signal-pulse" style="width:6px;height:6px;background:var(--status-building)"></span>
            {{ line.live_runs }} run{{ line.live_runs === 1 ? '' : 's' }} in flight
          </span>
          <span v-else-if="!line.valid" class="text-error truncate max-w-[420px]" :title="line.error">{{ line.error }}</span>
          <button
            class="text-ink-tertiary hover:text-signal transition-colors"
            @click="emit('duplicate')"
          >duplicate</button>
          <button
            class="text-ink-tertiary hover:text-signal transition-colors"
            title="Download this line, its stages and every workflow behind them as one file."
            @click="emit('export')"
          >export</button>
          <button
            class="text-ink-tertiary transition-colors"
            :class="locked ? 'opacity-40 cursor-not-allowed' : 'hover:text-signal'"
            :disabled="locked"
            :title="locked ? 'A run of this line is still walking it. Duplicate it to change it.' : ''"
            @click="startEdit"
          >edit</button>
          <button
            class="text-ink-tertiary hover:text-error transition-colors"
            @click="emit('delete')"
          >delete</button>
        </template>
      </div>
    </div>

    <!-- A line whose run is in flight cannot be edited. Said here, on load,
         rather than as a 409 after ten minutes of typing — and said next to
         the way out, which is to fork it. -->
    <div
      v-if="!editing && locked"
      class="flex flex-wrap items-center gap-3 border rounded px-3 py-2 font-mono text-[11px]"
      style="border-color: var(--status-degraded); color: var(--status-degraded)"
    >
      <span class="signal-dot" style="width:6px;height:6px;background:var(--status-degraded)"></span>
      <span>Editing is held while a run is walking this line — a live run reads its stages as it goes.</span>
      <button class="underline underline-offset-2 hover:text-signal" @click="emit('duplicate')">
        duplicate it to change it
      </button>
    </div>

    <!-- ===== Read-only: the route board ===================================
         Two terminals and the track between them, the way the contract board
         and the graph draw one — because a line is the same kind of fact. -->
    <div v-if="!editing && line" ref="boardScroller" class="card-ab p-6 overflow-x-auto">
      <!-- Three rows on one grid, so the labels, the track and the badges line
           up across every stage with no hand-tuned offsets: what each thing is
           on top, the track in the middle, what is set on it underneath. The
           joins take what is left over and the stages are fixed, so a short
           line fills the card and a long one scrolls rather than squeezing
           every name down to three letters. -->
      <div class="grid items-center gap-y-1" :style="boardColumns">
        <!-- What each column is. -->
        <span class="label">Accepts</span>
        <template v-for="(st, i) in stages" :key="`h${i}`">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-secondary text-center px-2 truncate">
            {{ i === 0 ? '' : joinLabel(i) }}
          </span>
          <span class="label px-0.5">St {{ i + 1 }}</span>
        </template>
        <span></span>
        <span class="label text-right">Produces</span>

        <!-- The track: a 1px run with a stop at each end, the way the contract
             board and the graph draw one. -->
        <span class="tag text-ink justify-self-start">{{ stages[0]?.accepts || '—' }}</span>
        <template v-for="(st, i) in stages" :key="`t${i}`">
          <span class="flex items-center self-stretch">
            <span
              v-if="i === 0"
              class="signal-dot"
              style="width:6px;height:6px;background:var(--border-strong)"
            ></span>
            <span class="flex-1 border-t border-line-strong"></span>
          </span>
          <span class="border border-line rounded-sm bg-base px-2.5 py-2 flex flex-col gap-0.5 min-w-0">
            <span class="font-mono text-xs font-medium text-ink truncate" :title="st.workflow_name">
              {{ st.workflow_name }}
            </span>
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary">
              {{ st.accepts }} → {{ st.produces }}
            </span>
          </span>
        </template>
        <span class="flex items-center self-stretch">
          <span class="flex-1 border-t border-line-strong"></span>
          <span class="signal-dot" style="width:6px;height:6px;background:var(--accent)"></span>
        </span>
        <span class="tag text-ink justify-self-end">
          {{ stages[stages.length - 1]?.produces || '—' }}
        </span>

        <!-- What is set on each stage. -->
        <span></span>
        <template v-for="(st, i) in stages" :key="`f${i}`">
          <span></span>
          <span class="flex flex-wrap items-center gap-2 px-0.5 font-mono text-[10px] uppercase tracking-[0.08em]">
            <span v-if="st.keep_output" class="text-signal">keep</span>
            <span v-if="st.hold_for_review" class="text-degraded">hold for review</span>
            <span v-if="askedCountOf(st)" class="text-ink-tertiary">{{ askedCountOf(st) }} asked</span>
            <span v-if="variedCountOf(st)" class="text-ink-tertiary">{{ variedCountOf(st) }} varied</span>
          </span>
        </template>
        <span></span>
        <span></span>
      </div>
    </div>

    <div v-if="!editing && line" class="flex items-baseline gap-4">
      <span v-if="line.description" class="text-[13px] font-light text-ink-secondary">
        {{ line.description }}
      </span>
      <span class="flex-1"></span>
      <span v-if="boardOverflows" class="font-mono text-[11px] text-ink-tertiary whitespace-nowrap">
        scrolls →
      </span>
    </div>

    <!-- ===== Editing: the vertical list =================================== -->
    <div v-if="editing" class="card-ab p-4 flex flex-col gap-0">
      <template v-for="(st, i) in draft.stages" :key="i">
        <!-- The connector above this stage. Clickable only when it asks. -->
        <div v-if="i > 0" class="flex items-stretch gap-3">
          <span class="w-8 flex-none flex justify-center">
            <span class="w-px bg-line-strong" style="min-height: 28px"></span>
          </span>
          <div class="flex flex-col gap-1 py-1 min-w-0 flex-1">
            <button
              class="self-start font-mono text-[11px] transition-colors"
              :class="joinOf(i)?.is_a_question
                ? 'text-ink-secondary hover:text-signal'
                : 'text-ink-tertiary cursor-default'"
              :disabled="!joinOf(i)?.is_a_question"
              :title="joinOf(i)?.is_a_question
                ? 'This join can be read more than one way'
                : 'Nothing to choose here'"
              @click="openConnector = openConnector === i ? null : i"
            >
              {{ joinLabel(i) }}<span v-if="joinOf(i)?.is_a_question"> ▾</span>
            </button>

            <div v-if="openConnector === i" class="flex flex-col gap-2 pb-1">
              <div v-if="(joinOf(i)?.modes || []).length" class="flex flex-wrap gap-2">
                <button
                  v-for="key in joinOf(i).modes"
                  :key="key"
                  class="whitespace-nowrap border rounded-sm px-2.5 py-1 font-mono text-[11px] transition-colors"
                  :class="modeKeyOf(i) === key
                    ? 'border-signal bg-surface text-signal'
                    : 'border-line text-ink-secondary hover:bg-raised'"
                  @click="setMode(i, key)"
                >{{ MODE_WORDS[key] }}</button>
                <span
                  v-if="modeKeyOf(i) === 'at_time'"
                  class="flex items-center gap-2"
                >
                  <input
                    :value="parseSourceMode(draft.stages[i].source_mode).ms"
                    type="number"
                    min="0"
                    step="100"
                    aria-label="Milliseconds into the clip"
                    class="w-24 bg-base border border-line rounded-sm px-2 py-1 font-mono text-[11px] text-ink"
                    @input="setModeMs(i, $event.target.value)"
                  />
                  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary">ms</span>
                </span>
              </div>

              <!-- Which slot the file lands in, when the stage has more than
                   one. FR2's own `role` directive; no second mechanism. -->
              <div v-if="(joinOf(i)?.roles || []).length > 1" class="flex flex-wrap items-center gap-2">
                <span class="label">Into slot</span>
                <button
                  v-for="role in joinOf(i).roles"
                  :key="role"
                  class="whitespace-nowrap border rounded-sm px-2.5 py-1 font-mono text-[11px] uppercase tracking-[0.08em] transition-colors"
                  :class="(roleOf(i) || joinOf(i).roles[0]) === role
                    ? 'border-signal bg-surface text-signal'
                    : 'border-line text-ink-secondary hover:bg-raised'"
                  @click="setRole(i, role)"
                >{{ role }}</button>
              </div>
            </div>
          </div>
        </div>

        <!-- The stage row -->
        <div class="flex items-start gap-3 py-1">
          <span
            class="w-8 h-8 flex-none flex items-center justify-center border border-line-strong rounded-sm font-mono text-[11px] text-ink"
          >{{ i + 1 }}</span>

          <div class="flex-1 min-w-0 flex flex-col gap-1">
            <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
              <button
                class="font-mono text-[13px] text-ink hover:text-signal transition-colors truncate max-w-full text-left"
                @click="openStage = openStage === i ? null : i"
              >{{ st.workflow_name }}</button>
              <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary">
                {{ st.accepts }} → {{ st.produces }}
              </span>
              <span class="flex-1"></span>
              <span class="flex items-center gap-3 font-mono text-[11px] text-ink-tertiary">
                <button
                  class="transition-colors disabled:opacity-30"
                  :disabled="i === 0"
                  title="Move up"
                  @click="move(i, -1)"
                >↑</button>
                <button
                  class="transition-colors disabled:opacity-30"
                  :disabled="i === draft.stages.length - 1"
                  title="Move down"
                  @click="move(i, 1)"
                >↓</button>
                <button class="hover:text-signal transition-colors" @click="openPicker(i, 'replace')">swap</button>
                <button class="hover:text-error transition-colors" @click="removeStage(i)">remove</button>
              </span>
            </div>

            <div class="flex flex-wrap items-center gap-4 font-mono text-[11px] text-ink-tertiary">
              <label class="flex items-center gap-1.5" title="Keep this stage's output once the run completes. The last stage's is kept regardless.">
                <input v-model="st.keep_output" type="checkbox" class="w-3 h-3 rounded-none border border-line bg-surface" style="accent-color: var(--accent)" />
                keep output
              </label>
              <!-- The one thing that turns a fan-out from four bills into one:
                   stop here, and let a person say which takes are worth the
                   stages below. Not offered on the last stage, whose output is
                   the product — the server refuses that, and offering it would
                   be offering a refusal. -->
              <label
                v-if="i < draft.stages.length - 1"
                class="flex items-center gap-1.5"
                title="Park the run after this stage and ask which of its takes go on. Its takes are always kept."
              >
                <input v-model="st.hold_for_review" type="checkbox" class="w-3 h-3 rounded-none border border-line bg-surface" style="accent-color: var(--status-degraded)" />
                hold for review
              </label>
              <span v-if="askedCountOf(st)" class="text-ink-secondary" :title="askedNames(st).join(', ')">
                {{ askedCountOf(st) }} asked at send time
              </span>
              <span v-if="variedCountOf(st)" class="text-signal">{{ variedCountOf(st) }} varied</span>
              <button class="hover:text-signal transition-colors" @click="openStage = openStage === i ? null : i">
                {{ openStage === i ? 'hide settings' : 'settings' }}
              </button>
            </div>

            <!-- The settings, each in one of the three dispositions. -->
            <div v-if="openStage === i" class="pt-1">
              <WorkflowInputControls
                v-if="inputsOf(st).length"
                v-model:text-overrides="st.text_overrides"
                v-model:parameters="st.parameters"
                v-model:vary="st.vary"
                v-model:exposed="st.exposed"
                :inputs="workflowOf(st)?.inputs || []"
                :loader-node-ids="loaderIdsOf(st)"
                allow-vary
                allow-expose
                @dirty="check"
              />
              <span v-else class="font-mono text-[11px] text-ink-tertiary">
                this workflow has nothing to set — the stage runs its author's own values
              </span>
            </div>
          </div>
        </div>
      </template>

      <!-- Add: only what fits after what is already there. -->
      <div class="flex items-start gap-3 pt-1">
        <span class="w-8 flex-none flex justify-center">
          <span
            class="w-px"
            style="min-height: 20px; background: repeating-linear-gradient(var(--border-strong) 0 3px, transparent 3px 6px)"
          ></span>
        </span>
        <button
          class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
          @click="openPicker(draft.stages.length, 'insert')"
        >+ add stage</button>
      </div>

      <!-- The picker -->
      <div v-if="picker" class="mt-3 border border-line rounded bg-base p-3 flex flex-col gap-2">
        <div class="flex items-baseline gap-3">
          <span class="label">{{ picker.mode === 'replace' ? `Swap stage ${picker.index + 1}` : 'Add stage' }}</span>
          <span class="flex-1"></span>
          <button class="font-mono text-[11px] text-ink-tertiary hover:text-signal" @click="picker = null">✕</button>
        </div>

        <div v-if="picker.loading" class="font-mono text-xs text-ink-tertiary">reading the library…</div>
        <div v-else-if="picker.error" class="font-mono text-xs text-error">{{ picker.error }}</div>
        <template v-else>
          <div class="flex flex-col">
            <button
              v-for="option in picker.items.filter((o) => o.offered)"
              :key="option.workflow_id"
              class="flex items-baseline gap-3 px-2 py-1.5 border-b border-line last:border-b-0 text-left hover:bg-raised transition-colors"
              @click="chooseStage(option)"
            >
              <span class="font-mono text-[13px] text-ink truncate min-w-0 flex-1">{{ option.name }}</span>
              <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary whitespace-nowrap">
                {{ option.accepts }} → {{ option.produces }}
              </span>
            </button>
            <div v-if="!picker.offered" class="px-2 py-3 font-mono text-xs text-ink-tertiary">
              nothing in this library fits here
            </div>
          </div>

          <div class="flex items-baseline gap-3">
            <span class="font-mono text-[11px] text-ink-tertiary">{{ pickerNote }}</span>
            <span class="flex-1"></span>
            <button
              v-if="picker.refused"
              class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              @click="picker.showRefused = !picker.showRefused"
            >{{ picker.showRefused ? 'hide' : 'why' }}</button>
          </div>

          <!-- What is not offered, and why. A picker that silently omitted half
               a library would read as a library that had lost half of it. -->
          <div v-if="picker.showRefused" class="flex flex-col gap-1 pt-1 border-t border-line">
            <div
              v-for="option in picker.items.filter((o) => !o.offered)"
              :key="option.workflow_id"
              class="flex flex-col px-2 py-1"
            >
              <span class="font-mono text-[11px] text-ink-tertiary">{{ option.name }}</span>
              <span class="font-mono text-[10px] text-ink-tertiary opacity-70">{{ option.reason }}</span>
            </div>
          </div>
        </template>
      </div>
    </div>

    <!-- Save -->
    <div v-if="editing" class="flex flex-wrap items-center gap-3">
      <button
        class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
        :disabled="!canSave"
        @click="save"
      >{{ saving ? 'Saving…' : isNew ? 'Save line' : 'Save changes' }}</button>
      <button class="font-mono text-[11px] text-ink-tertiary hover:text-ink transition-colors" @click="cancel">cancel</button>
      <span class="flex-1"></span>
      <span v-if="saveError" class="font-mono text-[11px] text-error truncate max-w-[480px]" :title="saveError">
        {{ saveError }}
      </span>
      <button
        v-if="saveConflict"
        class="font-mono text-[11px] text-signal hover:underline underline-offset-2"
        @click="emit('duplicate')"
      >save it as a fork instead</button>
    </div>
  </div>
</template>

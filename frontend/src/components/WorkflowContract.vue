<script setup>
/**
 * A workflow's stage contract, drawn as a route board.
 *
 * The graph beneath says what runs; this says what the workflow *is* — what
 * goes in one end and what comes out the other — because that is the only fact
 * that decides whether it can follow another workflow in a line.
 *
 * Everything on it is derived by heuristics that will sometimes be wrong: a
 * title that meant something else, a saver nobody could classify, a prompt box
 * wired somewhere unexpected. So every derived value is also a control. Two
 * clicks — open the picker, choose — and it is corrected for good; the
 * correction is stored beside the derivation and re-applied whenever Phos
 * works the contract out again.
 */
import { ref, computed, watch } from 'vue'

const props = defineProps({
  workflowId: { type: String, required: true },
  workflowName: { type: String, default: '' },
  /** The contract as the workflow list gave it. */
  contract: { type: Object, default: null },
})
const emit = defineEmits(['updated'])

const local = ref(null)
const saving = ref(false)
const error = ref('')

// An array of getters, not a getter returning an array: the second form makes
// a new array every run and so re-fires on every unrelated render.
watch(
  [() => props.workflowId, () => props.contract],
  () => {
    local.value = props.contract ? JSON.parse(JSON.stringify(props.contract)) : null
    error.value = ''
  },
  { immediate: true },
)

const c = computed(() => local.value)
const corrections = computed(() => c.value?.corrections || {})

const ACCEPTS = ['image', 'video', 'text', 'none']
const PRODUCES = ['image', 'video', 'text']
const ROLES = ['start', 'end', 'reference']
const PARAMS = ['seed', 'steps', 'cfg', 'denoise', 'frames', 'fps', 'width', 'height']
/** The two names a prompt compiler binds by. A third is nameable over the API. */
const SLOTS = ['positive', 'negative']

/**
 * What each warning means, in the register the rest of the console speaks.
 * The backend sends the code; the wording belongs to the screen showing it.
 */
const WARNINGS = {
  no_catalog:
    'ComfyUI could not be reached when this was worked out, so the settings carry no ranges. Phos tries again while it runs.',
  no_output_node: 'Nothing in this graph saves anything Phos recognises — the output type is a guess.',
  mixed_outputs: 'This graph saves more than one kind of thing. Check the output type.',
  unsupported_output: 'The only thing this graph saves is audio, which a line cannot carry.',
  no_source_loader: 'Nothing here reads a file, so this workflow can only begin a line.',
  unknown_classes:
    'This graph uses nodes that are not installed on this ComfyUI. They could not be typed, and a run would fail on them.',
}

function warningText(code) {
  return WARNINGS[code] || String(code).replace(/_/g, ' ')
}

/** `"<node_id>.<field>"`, the key everything is addressed by. */
function keyOf(entry) {
  return `${entry.node_id}.${entry.field}`
}

function isCorrected(kind, key) {
  const map = corrections.value
  if (kind === 'accepts') return map.accepts != null
  if (kind === 'produces') return map.produces != null
  return map[kind] != null && Object.prototype.hasOwnProperty.call(map[kind], key)
}

/**
 * PUTs are strictly serialised. Each request replaces the *whole* corrections
 * object, so two quick edits must not clone the same stale set — the second
 * would silently drop the first — and the replies must not land out of order.
 * The queue makes each edit read the corrections the previous response
 * returned, and the board shows the last reply, never an earlier one.
 */
let queue = Promise.resolve()
let inFlight = 0

function enqueue(build) {
  inFlight += 1
  saving.value = true
  queue = queue
    .then(() => put(build()))
    .catch(() => {})
    .finally(() => {
      inFlight -= 1
      if (inFlight === 0) saving.value = false
    })
}

/** The whole corrections object, with one entry changed, then PUT. */
function apply(mutate) {
  enqueue(() => {
    const next = JSON.parse(JSON.stringify(corrections.value || {}))
    next.roles ||= {}
    next.slots ||= {}
    next.params ||= {}
    mutate(next)
    return next
  })
}

async function put(next) {
  error.value = ''
  const res = await fetch(`/api/comfyui/workflows/${props.workflowId}/contract`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(next),
  }).catch((e) => {
    error.value = e.message || 'could not save the correction'
    throw e
  })
  if (!res.ok) {
    error.value = `HTTP ${res.status}`
    throw new Error(error.value)
  }
  local.value = await res.json()
  emit('updated')
}

const setAccepts = (v) => apply((n) => { n.accepts = v })
const setProduces = (v) => apply((n) => { n.produces = v })
const setRole = (nodeId, v) => apply((n) => { n.roles[nodeId] = v })
const clearRole = (nodeId) => apply((n) => { delete n.roles[nodeId] })
/** `null` is a real value here: "that text box is not a prompt". */
const setSlot = (key, v) => apply((n) => { n.slots[key] = v === '' ? null : v })
const clearSlot = (key) => apply((n) => { delete n.slots[key] })
const setParam = (key, v) => apply((n) => { n.params[key] = v === '' ? null : v })
const clearParam = (key) => apply((n) => { delete n.params[key] })

function clearAll() {
  enqueue(() => ({}))
}

/**
 * Rows a correction excluded ("not a prompt" / "not a setting") vanish from
 * the derived contract, but the *row* must stay on the board: its revert
 * button is the only way to undo that one exclusion without resetting
 * everything else. The excluded entries are synthesised back from the
 * corrections map, with the empty name the "not a…" option selects on.
 */
function withExcluded(rows, exclusions, extra) {
  const out = [...rows]
  const have = new Set(out.map(keyOf))
  for (const [key, val] of Object.entries(exclusions || {})) {
    if (val !== null || have.has(key)) continue
    const dot = key.indexOf('.')
    out.push({ name: '', node_id: key.slice(0, dot), field: key.slice(dot + 1), ...extra })
  }
  return out
}

const displaySlots = computed(() =>
  withExcluded(c.value?.slots || [], corrections.value.slots, { node_title: null, default: null }),
)
const displayParams = computed(() =>
  withExcluded(c.value?.params || [], corrections.value.params, { widget: null, current_value: null }),
)

const edited = computed(() => {
  const m = corrections.value
  return (
    m.accepts != null ||
    m.produces != null ||
    Object.keys(m.roles || {}).length > 0 ||
    Object.keys(m.slots || {}).length > 0 ||
    Object.keys(m.params || {}).length > 0
  )
})

/** A widget's range, said the way a schedule says one. */
function rangeOf(widget) {
  if (!widget) return ''
  const { kind, min, max, choices, multiline } = widget
  if (kind === 'combo') return `${(choices || []).length} choices`
  if (kind === 'text') return multiline ? 'text, multiline' : 'text'
  if (kind === 'boolean') return 'on / off'
  if (min == null && max == null) return kind || ''
  return `${min ?? '−∞'} – ${max ?? '∞'}`
}

function shortValue(v) {
  if (v === null || v === undefined) return '—'
  const s = typeof v === 'string' ? v : JSON.stringify(v)
  return s.length > 42 ? `${s.slice(0, 41)}…` : s
}
</script>

<template>
  <div v-if="c" class="flex flex-col gap-2 min-w-0">
    <div class="flex items-baseline justify-between gap-4">
      <div class="label">Contract</div>
      <div class="flex items-center gap-4 font-mono text-[11px]">
        <span v-if="saving" class="text-ink-tertiary">saving…</span>
        <span v-else-if="error" class="text-error">{{ error }}</span>
        <span v-else-if="edited" class="text-ink-tertiary">corrected by hand</span>
        <button
          v-if="edited"
          class="text-ink-tertiary hover:text-signal transition-colors"
          @click="clearAll"
        >reset all</button>
      </div>
    </div>

    <!-- The board. Two terminals and the track between them: everything a line
         needs to know before it can put this workflow after another one. -->
    <div class="card-ab p-6 overflow-x-auto">
      <div class="flex items-center gap-4 min-w-[520px]">
        <div class="flex flex-col gap-1.5 flex-none w-[132px] items-start">
          <span class="label">Accepts</span>
          <span class="relative inline-flex items-center">
            <select
              :value="c.accepts"
              aria-label="What this workflow accepts"
              class="tag appearance-none bg-base cursor-pointer transition-colors py-1 pr-6"
              :class="isCorrected('accepts') ? 'text-signal' : 'text-ink hover:text-signal'"
              @change="setAccepts($event.target.value)"
            >
              <option v-for="o in ACCEPTS" :key="o" :value="o">{{ o }}</option>
            </select>
            <span class="caret-ab">▾</span>
          </span>
        </div>

        <!-- The track, drawn the way the graph draws one: a 1px run with a
             solid stop at each end. -->
        <div class="flex-1 flex items-center min-w-[120px] pt-5">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--border-strong)"></span>
          <span class="flex-1 border-t border-line-strong"></span>
          <span class="px-3 font-mono text-[11px] text-ink-secondary truncate">{{ workflowName }}</span>
          <span class="flex-1 border-t border-line-strong"></span>
          <span class="signal-dot" style="width:6px;height:6px;background:var(--accent)"></span>
        </div>

        <div class="flex flex-col gap-1.5 flex-none w-[132px] items-end">
          <span class="label">Produces</span>
          <span class="relative inline-flex items-center">
            <select
              :value="c.produces"
              aria-label="What this workflow produces"
              class="tag appearance-none bg-base cursor-pointer transition-colors py-1 pr-6"
              :class="isCorrected('produces') ? 'text-signal' : 'text-ink hover:text-signal'"
              @change="setProduces($event.target.value)"
            >
              <option v-for="o in PRODUCES" :key="o" :value="o">{{ o }}</option>
            </select>
            <span class="caret-ab">▾</span>
          </span>
        </div>
      </div>
    </div>

    <!-- What the derivation was unsure about, said once, where it can be fixed. -->
    <div
      v-if="(c.warnings || []).length"
      class="border border-line rounded p-3 flex flex-col gap-1.5"
      style="border-color: var(--status-degraded)"
    >
      <div
        v-for="w in c.warnings"
        :key="w"
        class="flex items-start gap-2 text-xs font-light text-ink-secondary"
      >
        <span
          class="signal-dot mt-1"
          style="width:6px;height:6px;background:var(--status-degraded)"
        ></span>
        <span>{{ warningText(w) }}</span>
      </div>
    </div>

    <!-- Source slots: which loader takes which picture. -->
    <div v-if="(c.roles || []).length" class="flex flex-col gap-1 mt-2">
      <div class="label">Source slots</div>
      <div class="border border-line rounded overflow-hidden overflow-x-auto">
        <div
          class="grid gap-4 px-3 py-2 border-b min-w-[560px]"
          style="grid-template-columns: 56px 1fr 140px 1fr; border-color: var(--border-strong)"
        >
          <span class="label">Node</span>
          <span class="label">Type</span>
          <span class="label">Slot</span>
          <span class="label">Title</span>
        </div>
        <div
          v-for="r in c.roles"
          :key="r.node_id"
          class="grid gap-4 items-center px-3 py-1.5 border-b border-line font-mono text-xs min-w-[560px]"
          style="grid-template-columns: 56px 1fr 140px 1fr"
        >
          <span class="text-ink-tertiary">{{ r.node_id }}</span>
          <span class="text-ink-secondary truncate">
            {{ r.node_type }}
            <span class="text-ink-tertiary uppercase">· {{ r.kind }}</span>
          </span>
          <span class="flex items-center gap-2">
            <span class="relative inline-flex items-center">
              <select
                :value="r.role"
                :aria-label="`Which slot node ${r.node_id} fills`"
                class="appearance-none bg-base border border-line rounded-sm pl-1.5 pr-5 py-0.5 text-[11px] uppercase tracking-[0.08em] cursor-pointer transition-colors hover:border-signal"
                :class="isCorrected('roles', r.node_id) ? 'text-signal' : 'text-ink'"
                @change="setRole(r.node_id, $event.target.value)"
              >
                <option v-for="o in ROLES" :key="o" :value="o">{{ o }}</option>
              </select>
              <span class="caret-ab">▾</span>
            </span>
            <button
              v-if="isCorrected('roles', r.node_id)"
              class="text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              @click="clearRole(r.node_id)"
            >revert</button>
          </span>
          <span class="text-ink-tertiary truncate">{{ r.title || '—' }}</span>
        </div>
      </div>
    </div>

    <!-- Prompt slots: what a person, or a describe stage, writes into. -->
    <div v-if="displaySlots.length" class="flex flex-col gap-1 mt-2">
      <div class="label">Prompt slots</div>
      <div class="border border-line rounded overflow-hidden overflow-x-auto">
        <div
          class="grid gap-4 px-3 py-2 border-b min-w-[560px]"
          style="grid-template-columns: 56px 1fr 160px 1fr; border-color: var(--border-strong)"
        >
          <span class="label">Node</span>
          <span class="label">Field</span>
          <span class="label">Slot</span>
          <span class="label">Author's text</span>
        </div>
        <div
          v-for="s in displaySlots"
          :key="keyOf(s)"
          class="grid gap-4 items-center px-3 py-1.5 border-b border-line font-mono text-xs min-w-[560px]"
          style="grid-template-columns: 56px 1fr 160px 1fr"
        >
          <span class="text-ink-tertiary">{{ s.node_id }}</span>
          <span class="text-ink-secondary truncate">
            {{ s.field }}
            <span v-if="s.node_title" class="text-ink-tertiary">· {{ s.node_title }}</span>
          </span>
          <span class="flex items-center gap-2">
            <span class="relative inline-flex items-center min-w-0">
              <select
                :value="s.name"
                :aria-label="`What ${keyOf(s)} is`"
                class="appearance-none bg-base border border-line rounded-sm pl-1.5 pr-5 py-0.5 text-[11px] uppercase tracking-[0.08em] cursor-pointer transition-colors hover:border-signal min-w-0 max-w-[132px]"
                :class="isCorrected('slots', keyOf(s)) ? 'text-signal' : 'text-ink'"
                @change="setSlot(keyOf(s), $event.target.value)"
              >
                <option v-if="s.name && !SLOTS.includes(s.name)" :value="s.name">{{ s.name }}</option>
                <option v-for="o in SLOTS" :key="o" :value="o">{{ o }}</option>
                <option value="">not a prompt</option>
              </select>
              <span class="caret-ab">▾</span>
            </span>
            <button
              v-if="isCorrected('slots', keyOf(s))"
              class="text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              @click="clearSlot(keyOf(s))"
            >revert</button>
          </span>
          <span class="text-ink-tertiary truncate" :title="s.default || ''">{{ shortValue(s.default) }}</span>
        </div>
      </div>
    </div>

    <!-- Parameters: the settings a line can dial without knowing the graph. -->
    <div v-if="displayParams.length" class="flex flex-col gap-1 mt-2">
      <div class="label">Parameters</div>
      <div class="border border-line rounded overflow-hidden overflow-x-auto">
        <div
          class="grid gap-4 px-3 py-2 border-b min-w-[600px]"
          style="grid-template-columns: 56px 1fr 150px 150px 100px; border-color: var(--border-strong)"
        >
          <span class="label">Node</span>
          <span class="label">Field</span>
          <span class="label">Setting</span>
          <span class="label">Range</span>
          <span class="label">Value</span>
        </div>
        <div
          v-for="p in displayParams"
          :key="keyOf(p)"
          class="grid gap-4 items-center px-3 py-1.5 border-b border-line font-mono text-xs min-w-[600px]"
          style="grid-template-columns: 56px 1fr 150px 150px 100px"
        >
          <span class="text-ink-tertiary">{{ p.node_id }}</span>
          <span class="text-ink-secondary truncate">{{ p.field }}</span>
          <span class="flex items-center gap-2">
            <span class="relative inline-flex items-center min-w-0">
              <select
                :value="p.name"
                :aria-label="`What ${keyOf(p)} sets`"
                class="appearance-none bg-base border border-line rounded-sm pl-1.5 pr-5 py-0.5 text-[11px] uppercase tracking-[0.08em] cursor-pointer transition-colors hover:border-signal max-w-[112px]"
                :class="isCorrected('params', keyOf(p)) ? 'text-signal' : 'text-ink'"
                @change="setParam(keyOf(p), $event.target.value)"
              >
                <option v-for="o in PARAMS" :key="o" :value="o">{{ o }}</option>
                <option value="">not a setting</option>
              </select>
              <span class="caret-ab">▾</span>
            </span>
            <button
              v-if="isCorrected('params', keyOf(p))"
              class="text-[11px] text-ink-tertiary hover:text-signal transition-colors"
              @click="clearParam(keyOf(p))"
            >revert</button>
          </span>
          <span class="text-ink-tertiary truncate">{{ rangeOf(p.widget) || 'untyped' }}</span>
          <span class="text-ink-secondary truncate">{{ shortValue(p.current_value) }}</span>
        </div>
      </div>
    </div>

    <div class="font-mono text-[11px] text-ink-tertiary">
      Worked out from the graph and what ComfyUI says about its nodes. Anything wrong here is a
      pick away from being right, and stays right the next time Phos reads the workflow.
    </div>
  </div>
</template>

<style scoped>
/* Native select arrows differ by platform and none of them are on-system, so
   they are replaced with the one glyph the rest of the console uses. */
.caret-ab {
  position: absolute;
  right: 6px;
  pointer-events: none;
  font-size: 9px;
  line-height: 1;
  color: var(--text-tertiary);
}
</style>

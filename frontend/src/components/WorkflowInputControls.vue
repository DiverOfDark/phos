<script setup>
/**
 * A workflow's editable inputs, each rendered as what it actually is.
 *
 * Laid out as a schedule board rather than a settings panel: one 1px-ruled row
 * per field, the node id / class / field name in uppercase mono on the left, the
 * kind of field on the right, the control underneath. Nothing here introduces a
 * colour — signal amber marks the one thing that is active in a row, status
 * colour says a value cannot be read, and that is the whole palette.
 *
 * Two override channels, matching the backend: text and anything the catalogue
 * could not type go into `textOverrides`; seeds, counts, scales, switches and
 * model pickers go into `parameters`, each keeping its own JSON type. `vary`
 * carries the sweeps — a parameter set to run several values rather than one.
 */
import { ref, computed, watch, onMounted } from 'vue'
import {
  controlKind,
  kindLabel,
  inputKey,
  numberBounds,
  parameterValue,
  parseValueList,
  randomSeed,
} from '@/lib/utils'

const props = defineProps({
  inputs: { type: Array, default: () => [] },
  /** Node ids the source picker already fills — never asked for as a control. */
  loaderNodeIds: { type: Array, default: () => [] },
  /** Presets pin values; only a queue request can sweep them. */
  allowVary: { type: Boolean, default: false },
})

const emit = defineEmits(['dirty'])

const textOverrides = defineModel('textOverrides', { type: Object, default: () => ({}) })
const parameters = defineModel('parameters', { type: Object, default: () => ({}) })
const vary = defineModel('vary', { type: Object, default: () => ({}) })

// --- Which inputs get a row ------------------------------------------------

function isLoaderInput(input) {
  if (input.node_type === 'LoadImage') return true
  return props.loaderNodeIds.includes(input.node_id)
}

const rows = computed(() =>
  props.inputs.filter((i) => !isLoaderInput(i) && controlKind(i) !== null),
)

// --- Enum choices ----------------------------------------------------------
// A combo listing every checkpoint on a loaded server is capped before it is
// stored beside the workflow. Where that happened, ask the live catalogue once
// for the full list rather than offering a menu that quietly omits a model.

const liveChoices = ref({})

function choicesFor(input) {
  const choices = (liveChoices.value[inputKey(input)] || input.widget?.choices || []).map(String)
  // The value the workflow or preset already holds is always among them. A
  // snapshot lists at most a few hundred choices, and a model can be
  // uninstalled after the workflow was imported — a <select> that cannot show
  // its own value renders as something else while the stored value stays put.
  const current = parameters[inputKey(input)] ?? parameterValue(input)
  if (typeof current === 'string' && current !== '' && !choices.includes(current)) {
    return [current, ...choices]
  }
  return choices
}

onMounted(async () => {
  const classes = [
    ...new Set(rows.value.filter((i) => i.widget?.truncated).map((i) => i.node_type)),
  ]
  if (!classes.length) return
  try {
    const res = await fetch(`/api/comfyui/nodes?classes=${encodeURIComponent(classes.join(','))}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    if (!data.available) return
    const found = {}
    for (const input of rows.value) {
      const declared = (data.nodes?.[input.node_type]?.inputs || []).find(
        (d) => d.name === input.field_name,
      )
      if (declared?.widget?.choices) found[inputKey(input)] = declared.widget.choices
    }
    liveChoices.value = found
  } catch (e) {
    // The stored list is a real list, just a shorter one. Nothing to say.
    console.warn('Could not read the full enum lists', e)
  }
})

// --- Editing ---------------------------------------------------------------

function setText(input, value) {
  textOverrides.value = { ...textOverrides.value, [inputKey(input)]: value }
  emit('dirty')
}

function setParameter(input, raw) {
  parameters.value = { ...parameters.value, [inputKey(input)]: parameterValue(input, raw) }
  emit('dirty')
}

function reroll(input) {
  setParameter(input, randomSeed(input))
}

// --- Sweeps ----------------------------------------------------------------

const SEED_POLICIES = [
  { key: 'fixed', note: 'run the seed in the box' },
  { key: 'random', note: 'draw a fresh seed per run' },
  { key: 'increment', note: 'the seed in the box, then the next, and the next' },
]

function seedPolicy(input) {
  return vary.value[inputKey(input)]?.mode || 'fixed'
}

function setSeedPolicy(input, policy) {
  const key = inputKey(input)
  const next = { ...vary.value }
  if (policy === 'fixed') {
    delete next[key]
  } else {
    const { min, max } = numberBounds(input)
    next[key] = { count: next[key]?.count || 2, mode: policy, min, max }
  }
  vary.value = next
  emit('dirty')
}

function setSweepCount(input, count) {
  const key = inputKey(input)
  if (!vary.value[key]) return
  const n = Math.max(1, Math.trunc(Number(count) || 1))
  vary.value = { ...vary.value, [key]: { ...vary.value[key], count: n } }
  emit('dirty')
}

/** Raw text of a value list, per key, so half-typed input is not thrown away. */
const sweepText = ref({})

/**
 * Is this row showing a value list?
 *
 * Deliberately *not* "does `vary` have this key": a half-typed or unreadable
 * list is not an axis, but the field it was typed into must stay on screen —
 * dropping the control mid-keystroke is how a person loses what they wrote.
 */
function isSweeping(input) {
  const key = inputKey(input)
  return key in sweepText.value || key in vary.value
}

function toggleSweep(input) {
  const key = inputKey(input)
  const next = { ...vary.value }
  if (isSweeping(input)) {
    delete next[key]
    vary.value = next
    const text = { ...sweepText.value }
    delete text[key]
    sweepText.value = text
    emit('dirty')
    return
  }
  // Seed it with the value already in the box, so the field is never empty.
  const current = parameters.value[key] ?? parameterValue(input)
  sweepText.value = { ...sweepText.value, [key]: String(current) }
  next[key] = { values: [current] }
  vary.value = next
  emit('dirty')
}

function setSweepValues(input, text) {
  const key = inputKey(input)
  sweepText.value = { ...sweepText.value, [key]: text }
  const values = parseValueList(text, input)
  const next = { ...vary.value }
  if (values) {
    next[key] = { values }
  } else {
    // Unreadable: drop the axis so the run count tells the truth, and let the
    // row say why rather than queueing something nobody asked for.
    delete next[key]
  }
  vary.value = next
  emit('dirty')
}

function sweepError(input) {
  const key = inputKey(input)
  if (!(key in sweepText.value)) return ''
  if (key in vary.value) return ''
  return controlKind(input) === 'combo'
    ? 'not names this server has'
    : 'expected numbers separated by commas'
}

function sweepSize(input) {
  const axis = vary.value[inputKey(input)]
  if (!axis) return 0
  return Array.isArray(axis.values) ? axis.values.length : axis.count || 1
}

// A workflow change replaces the maps wholesale; forget the half-typed text too.
watch(
  () => props.inputs,
  () => {
    sweepText.value = {}
  },
)

defineExpose({ rows })
</script>

<template>
  <div v-if="rows.length" class="border border-line rounded bg-base">
    <div
      v-for="(input, index) in rows"
      :key="inputKey(input)"
      class="flex flex-col gap-1.5 px-3 py-2.5"
      :class="index ? 'border-t border-line' : ''"
    >
      <!-- Label line: what this field is, and what kind of thing it holds. -->
      <div class="flex items-baseline gap-2">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary truncate">
          {{ input.node_id }} · {{ input.node_title || input.node_type }} · {{ input.field_name }}
        </span>
        <span class="flex-1"></span>
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary whitespace-nowrap">
          {{ kindLabel(input) }}
        </span>
      </div>

      <!-- Text: the channel that always existed. -->
      <textarea
        v-if="controlKind(input) === 'textarea'"
        :value="textOverrides[inputKey(input)] ?? ''"
        :placeholder="String(input.current_value ?? '')"
        rows="2"
        spellcheck="false"
        class="w-full bg-surface border border-line rounded-sm px-3 py-2 font-mono text-xs text-ink"
        @input="setText(input, $event.target.value)"
      ></textarea>
      <input
        v-else-if="controlKind(input) === 'text'"
        :value="textOverrides[inputKey(input)] ?? ''"
        :placeholder="String(input.current_value ?? '')"
        type="text"
        spellcheck="false"
        class="w-full bg-surface border border-line rounded-sm px-3 py-1.5 font-mono text-xs text-ink"
        @input="setText(input, $event.target.value)"
      />

      <!-- Seed: a number, plus what to do with it across a run. -->
      <template v-else-if="controlKind(input) === 'seed'">
        <div class="flex flex-wrap items-center gap-2">
          <input
            :value="parameters[inputKey(input)] ?? parameterValue(input)"
            type="number"
            :min="numberBounds(input).min"
            :max="numberBounds(input).max"
            step="1"
            :disabled="seedPolicy(input) === 'random'"
            class="w-52 bg-surface border border-line rounded-sm px-3 py-1.5 font-mono text-xs text-ink disabled:opacity-40"
            @input="setParameter(input, $event.target.value)"
          />
          <button
            v-if="seedPolicy(input) !== 'random'"
            title="draw a new seed"
            class="border border-line rounded-sm px-2 py-1.5 font-mono text-xs text-ink-tertiary hover:text-signal transition-colors"
            @click="reroll(input)"
          >↻</button>
          <div v-if="allowVary" class="flex">
            <button
              v-for="(policy, p) in SEED_POLICIES"
              :key="policy.key"
              :title="policy.note"
              class="border border-line px-2.5 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] transition-colors"
              :class="[
                p === 0 ? 'rounded-l-sm' : '-ml-px',
                p === SEED_POLICIES.length - 1 ? 'rounded-r-sm' : '',
                seedPolicy(input) === policy.key
                  ? 'border-signal bg-surface text-signal relative z-10'
                  : 'text-ink-tertiary hover:text-ink-secondary',
              ]"
              @click="setSeedPolicy(input, policy.key)"
            >{{ policy.key }}</button>
          </div>
          <template v-if="allowVary && seedPolicy(input) !== 'fixed'">
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary">runs</span>
            <input
              :value="vary[inputKey(input)]?.count ?? 1"
              type="number"
              min="1"
              max="64"
              step="1"
              class="w-16 bg-surface border border-line rounded-sm px-2 py-1.5 font-mono text-xs text-ink"
              @input="setSweepCount(input, $event.target.value)"
            />
          </template>
        </div>
      </template>

      <!-- Boolean: a switch, not a dropdown of "true" and "false". -->
      <label
        v-else-if="controlKind(input) === 'boolean'"
        class="flex items-center gap-2 font-mono text-xs text-ink-secondary cursor-pointer"
      >
        <input
          :checked="parameters[inputKey(input)] ?? parameterValue(input)"
          type="checkbox"
          class="w-3.5 h-3.5 rounded-none border border-line bg-surface"
          style="accent-color: var(--accent)"
          @change="setParameter(input, $event.target.checked)"
        />
        {{ (parameters[inputKey(input)] ?? parameterValue(input)) ? 'on' : 'off' }}
      </label>

      <!-- Numbers and enums: one control, or a list of values to sweep. -->
      <template v-else>
        <div class="flex flex-wrap items-center gap-2">
          <template v-if="!isSweeping(input)">
            <select
              v-if="controlKind(input) === 'combo'"
              :value="parameters[inputKey(input)] ?? parameterValue(input)"
              class="min-w-0 max-w-full bg-surface border border-line rounded-sm px-2 py-1.5 font-mono text-xs text-ink"
              @change="setParameter(input, $event.target.value)"
            >
              <option v-for="choice in choicesFor(input)" :key="choice" :value="choice">{{ choice }}</option>
            </select>
            <input
              v-else
              :value="parameters[inputKey(input)] ?? parameterValue(input)"
              type="number"
              :min="numberBounds(input).min ?? undefined"
              :max="numberBounds(input).max ?? undefined"
              :step="numberBounds(input).step"
              class="w-32 bg-surface border border-line rounded-sm px-3 py-1.5 font-mono text-xs text-ink"
              @input="setParameter(input, $event.target.value)"
            />
            <span
              v-if="numberBounds(input).min !== null && controlKind(input) !== 'combo'"
              class="font-mono text-[10px] text-ink-tertiary"
            >{{ numberBounds(input).min }}–{{ numberBounds(input).max }}</span>
          </template>

          <template v-else>
            <input
              :value="sweepText[inputKey(input)] ?? ''"
              :list="`choices-${inputKey(input)}`"
              type="text"
              spellcheck="false"
              placeholder="4, 6, 8"
              class="flex-1 min-w-40 bg-surface border rounded-sm px-3 py-1.5 font-mono text-xs text-ink"
              :class="sweepError(input) ? 'border-error' : 'border-signal'"
              @input="setSweepValues(input, $event.target.value)"
            />
            <datalist v-if="controlKind(input) === 'combo'" :id="`choices-${inputKey(input)}`">
              <option v-for="choice in choicesFor(input)" :key="choice" :value="choice"></option>
            </datalist>
            <span
              v-if="sweepError(input)"
              class="font-mono text-[10px] whitespace-nowrap"
              style="color: var(--status-error)"
            >{{ sweepError(input) }}</span>
            <span v-else class="font-mono text-[10px] text-signal whitespace-nowrap">×{{ sweepSize(input) }}</span>
          </template>

          <button
            v-if="allowVary"
            :title="isSweeping(input) ? 'run one value' : 'run several values, one task each'"
            class="border rounded-sm px-2.5 py-1.5 font-mono text-[10px] uppercase tracking-[0.08em] transition-colors"
            :class="isSweeping(input)
              ? 'border-signal text-signal'
              : 'border-line text-ink-tertiary hover:text-ink-secondary'"
            @click="toggleSweep(input)"
          >vary</button>
        </div>
      </template>
    </div>
  </div>
</template>

<script setup>
/**
 * The confirm sheet: the last thing between a person and forty-one hours of GPU.
 *
 * It reads as a departure board, because that is what it is — a scheduled
 * quantity of work, with the numbers in the mono register and nothing rounded
 * away. Four rows in a fixed order, largest number first, each one narrowing
 * the one above it:
 *
 *   12,431 shots matched
 *    9,102 already have output from this line          [skip] [redo]
 *    3,329 to run  ·  ×2 seeds  =  6,658 tasks
 *    est. 41 h GPU  ·  est. 780 GB disk
 *
 * Nothing is queued until Send. The preview endpoint writes nothing, so
 * flipping skip/redo or moving a cap re-asks it and costs nothing but a query.
 */
import { ref, computed, watch } from 'vue'
import {
  formatCount,
  formatGpu,
  formatBytes,
  formatWindow,
  fanoutSummary,
  estimateCaveats,
  minutesFromTime,
  capsPayload,
  selectionShorthand,
} from '../lib/batches.js'

const props = defineProps({
  open: { type: Boolean, default: false },
  /** `{ kind: 'query' | 'ids', ... }` */
  selection: { type: Object, default: null },
  lines: { type: Array, default: () => [] },
  /** Pre-select this line, e.g. from a saved selection. */
  lineId: { type: String, default: '' },
})
const emit = defineEmits(['close', 'sent'])

const selectedLineId = ref('')
const skipIfGenerated = ref(true)
const preview = ref(null)
const previewing = ref(false)
const error = ref('')
const sending = ref(false)
const showCaps = ref(false)
const saveName = ref('')
const saving = ref(false)
const saved = ref(false)

/** The caps, as a person fills them in. Empty means "no cap of this kind". */
const caps = ref({
  dailyTaskCap: '',
  windowEnabled: false,
  windowStart: '00:00',
  windowEnd: '07:00',
  diskFloorGb: '',
  maxOutstandingHolds: '',
})

const selectedLine = computed(() =>
  props.lines.find((l) => l.id === selectedLineId.value) || null
)

/** A window half filled in paces nothing, so the sheet says so before Send. */
const windowBroken = computed(
  () =>
    caps.value.windowEnabled &&
    (minutesFromTime(caps.value.windowStart) === null ||
      minutesFromTime(caps.value.windowEnd) === null)
)

const caveats = computed(() => estimateCaveats(preview.value))
const fanout = computed(() => fanoutSummary(preview.value?.stages))
const canSend = computed(
  () =>
    !!selectedLineId.value &&
    !!preview.value &&
    !preview.value.refused &&
    preview.value.to_run > 0 &&
    !windowBroken.value &&
    !sending.value
)

function reset() {
  preview.value = null
  error.value = ''
  saved.value = false
  saveName.value = ''
}

watch(
  () => props.open,
  (open) => {
    if (!open) return
    reset()
    selectedLineId.value = props.lineId || props.lines[0]?.id || ''
    refresh()
  }
)

watch([selectedLineId, skipIfGenerated], refresh)

function body() {
  return {
    line_id: selectedLineId.value,
    selection: props.selection,
    skip_if_generated: skipIfGenerated.value,
    caps: capsPayload(caps.value),
  }
}

async function refresh() {
  if (!props.open || !selectedLineId.value || !props.selection) return
  previewing.value = true
  error.value = ''
  try {
    const res = await fetch('/api/comfyui/batches/preview', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body()),
    })
    const data = await res.json().catch(() => ({}))
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)
    preview.value = data
  } catch (e) {
    console.error('Failed to preview batch', e)
    error.value = e.message
    preview.value = null
  } finally {
    previewing.value = false
  }
}

async function send() {
  if (!canSend.value) return
  sending.value = true
  error.value = ''
  try {
    const res = await fetch('/api/comfyui/batches', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body()),
    })
    const data = await res.json().catch(() => ({}))
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)
    emit('sent', data)
    emit('close')
  } catch (e) {
    console.error('Failed to send batch', e)
    error.value = e.message
  } finally {
    sending.value = false
  }
}

/** A query plus the line you usually send it to. It never fires on its own. */
async function saveSelection() {
  if (!saveName.value.trim()) return
  saving.value = true
  try {
    const res = await fetch('/api/comfyui/selections', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: saveName.value.trim(),
        line_id: selectedLineId.value || null,
        selection: props.selection,
        skip_if_generated: skipIfGenerated.value,
        caps: capsPayload(caps.value),
      }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    saved.value = true
  } catch (e) {
    console.error('Failed to save selection', e)
    error.value = e.message
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-50 flex items-center justify-center p-4"
    style="background: var(--scrim)"
    @click="emit('close')"
  >
    <div
      class="w-[560px] max-w-full max-h-[calc(100vh-64px)] bg-overlay border border-line-strong rounded flex flex-col overflow-hidden"
      role="dialog"
      aria-label="Send to a line"
      @click.stop
    >
      <div class="flex items-center justify-between px-6 py-4 border-b border-line">
        <div class="flex flex-col gap-0.5 min-w-0">
          <div class="font-heading text-base font-semibold text-ink">Send to a line</div>
          <div class="font-mono text-[11px] text-ink-tertiary truncate uppercase tracking-[0.08em]">
            {{ selectionShorthand(selection) }}
          </div>
        </div>
        <button
          class="font-mono text-[13px] text-ink-tertiary hover:text-signal"
          aria-label="Close"
          @click="emit('close')"
        >✕</button>
      </div>

      <div class="p-6 flex flex-col gap-6 overflow-y-auto min-h-0">
        <!-- Which line. Everything below re-reads when this changes, because
             what is already generated and what a stage costs are both facts
             about the line, not about the selection. -->
        <div class="flex flex-col gap-2">
          <div class="label">Line</div>
          <div v-if="!lines.length" class="text-[13px] font-light text-ink-secondary">
            No lines yet — build one in Workflows › Lines, or install a template.
          </div>
          <div v-else class="flex flex-wrap gap-2">
            <button
              v-for="ln in lines"
              :key="ln.id"
              class="whitespace-nowrap border rounded px-3 py-1.5 font-mono text-xs transition-colors"
              :class="selectedLineId === ln.id
                ? 'border-signal bg-surface text-signal'
                : 'border-line text-ink-secondary hover:bg-raised'"
              @click="selectedLineId = ln.id"
            >
              {{ ln.name }}
              <span class="text-ink-tertiary">· {{ (ln.stages || []).length }}</span>
            </button>
          </div>
        </div>

        <!-- The board. Four rows, largest first, each narrowing the one above. -->
        <div class="card-ab">
          <div
            v-if="previewing && !preview"
            class="px-4 py-6 font-mono text-xs text-ink-tertiary"
          >counting…</div>

          <div v-else-if="!preview" class="px-4 py-6 font-mono text-xs text-ink-tertiary">
            {{ error || 'pick a line' }}
          </div>

          <template v-else>
            <!-- Matched -->
            <div class="flex items-baseline gap-3 px-4 py-2.5 border-b border-line">
              <span class="font-mono text-[22px] tabular-nums text-ink w-[104px] text-right">
                {{ formatCount(preview.matched) }}
              </span>
              <span class="text-[13px] text-ink-secondary">shots matched</span>
            </div>

            <!-- Already done. The skip/redo switch lives on the row it changes. -->
            <div class="flex items-baseline gap-3 px-4 py-2.5 border-b border-line">
              <span
                class="font-mono text-[22px] tabular-nums w-[104px] text-right"
                :class="skipIfGenerated ? 'text-ink-tertiary' : 'text-ink'"
              >{{ formatCount(preview.skipped) }}</span>
              <span class="text-[13px] text-ink-secondary flex-1 min-w-0">
                already have output from this line
              </span>
              <div class="flex border border-line rounded overflow-hidden shrink-0">
                <button
                  v-for="opt in [
                    { id: true, label: 'skip' },
                    { id: false, label: 'redo' },
                  ]"
                  :key="String(opt.id)"
                  class="px-2.5 py-1 font-mono text-[11px] uppercase tracking-[0.08em] transition-colors"
                  :class="skipIfGenerated === opt.id
                    ? 'bg-signal text-signal-fg'
                    : 'text-ink-tertiary hover:text-ink hover:bg-raised'"
                  @click="skipIfGenerated = opt.id"
                >{{ opt.label }}</button>
              </div>
            </div>

            <!-- What actually runs, and what it comes to in tasks. -->
            <div class="flex items-baseline gap-3 px-4 py-2.5 border-b border-line">
              <span class="font-mono text-[22px] tabular-nums text-signal w-[104px] text-right">
                {{ formatCount(preview.to_run) }}
              </span>
              <!-- The task count only earns its place when it differs from the
                   shot count. A line with no sweep would otherwise read
                   "240 to run = 240 tasks", which says the same thing twice. -->
              <span class="text-[13px] text-ink-secondary flex items-baseline gap-2 flex-wrap">
                <span>to run</span>
                <template v-if="preview.tasks !== preview.to_run">
                  <span v-if="fanout" class="font-mono text-[11px] text-ink-tertiary">· {{ fanout }}</span>
                  <span class="font-mono text-[11px] text-ink-tertiary">=</span>
                  <span class="font-mono text-[13px] tabular-nums text-ink">{{ formatCount(preview.tasks) }}</span>
                  <span>tasks</span>
                </template>
              </span>
            </div>

            <!-- What it costs. -->
            <div class="flex items-baseline gap-3 px-4 py-2.5" :class="caveats.length ? 'border-b border-line' : ''">
              <span class="label w-[104px] text-right">est.</span>
              <span class="flex items-baseline gap-2 flex-wrap font-mono text-[13px] tabular-nums text-ink">
                <span>{{ formatGpu(preview.gpu_seconds) }}</span>
                <span class="text-[11px] text-ink-tertiary uppercase tracking-[0.08em]">GPU</span>
                <span class="text-ink-tertiary">·</span>
                <span>{{ formatBytes(preview.disk_bytes) }}</span>
                <span class="text-[11px] text-ink-tertiary uppercase tracking-[0.08em]">disk</span>
              </span>
            </div>

            <!-- Where the numbers are soft, said plainly rather than hidden. -->
            <div v-if="caveats.length" class="px-4 py-2.5 flex flex-col gap-1">
              <div
                v-for="c in caveats"
                :key="c"
                class="text-[12px] font-light"
                style="color: var(--status-degraded)"
              >{{ c }}</div>
            </div>
          </template>
        </div>

        <!-- Per-stage cost, folded away. The sheet's headline is four numbers;
             this is for the person who wants to know which stage the 41 hours
             is in, and which of them Phos has actually measured. -->
        <details v-if="preview && preview.stages?.length" class="flex flex-col gap-2">
          <summary class="label cursor-pointer select-none hover:text-ink-secondary">
            Per stage
          </summary>
          <div class="card-ab mt-2 overflow-x-auto">
            <div
              class="grid gap-3 px-4 py-2 border-b min-w-[440px]"
              style="grid-template-columns: 28px minmax(0,1fr) 56px 76px 76px 84px; border-color: var(--border-strong)"
            >
              <span class="label">#</span>
              <span class="label">Workflow</span>
              <span class="label">Fan</span>
              <span class="label">Each</span>
              <span class="label">Size</span>
              <span class="label">Source</span>
            </div>
            <div
              v-for="st in preview.stages"
              :key="st.stage_idx"
              class="grid gap-3 items-baseline px-4 py-2 border-b border-line last:border-0 min-w-[440px]"
              style="grid-template-columns: 28px minmax(0,1fr) 56px 76px 76px 84px"
            >
              <span class="font-mono text-[11px] text-ink-tertiary tabular-nums">{{ st.stage_idx + 1 }}</span>
              <span class="text-[13px] text-ink truncate flex items-baseline gap-2">
                {{ st.workflow_name }}
                <span
                  v-if="st.holds"
                  class="font-mono text-[10px] uppercase tracking-[0.08em] shrink-0"
                  style="color: var(--status-degraded)"
                >hold</span>
              </span>
              <span class="font-mono text-[11px] tabular-nums" :class="st.fanout > 1 ? 'text-signal' : 'text-ink-tertiary'">
                ×{{ st.fanout }}
              </span>
              <span class="font-mono text-[11px] text-ink-secondary tabular-nums">
                {{ formatGpu(st.seconds_per_task) }}
              </span>
              <span class="font-mono text-[11px] tabular-nums" :class="st.keeps_output ? 'text-ink-secondary' : 'text-ink-tertiary line-through'">
                {{ formatBytes(st.bytes_per_task) }}
              </span>
              <span
                class="font-mono text-[10px] uppercase tracking-[0.08em]"
                :style="{ color: st.measured ? 'var(--status-ready)' : 'var(--status-degraded)' }"
              >{{ st.measured ? 'measured' : 'guess' }}</span>
            </div>
          </div>
          <div class="text-[12px] font-light text-ink-secondary mt-2">
            A struck-through size is an intermediate the run sweeps when it lands, so it
            costs disk while the batch is going and nothing afterwards.
          </div>
        </details>

        <!-- Caps. Folded because most batches want none, and open by one click
             because the batches that want them want them badly. -->
        <div class="flex flex-col gap-3">
          <button
            class="label text-left hover:text-ink-secondary flex items-center gap-2"
            @click="showCaps = !showCaps"
          >
            <span>{{ showCaps ? '▾' : '▸' }}</span>
            Caps
            <span v-if="!showCaps && Object.keys(capsPayload(caps)).length" class="text-signal normal-case tracking-normal">
              · {{ Object.keys(capsPayload(caps)).length }} set
            </span>
          </button>

          <div v-if="showCaps" class="flex flex-col gap-4 pl-4 border-l border-line">
            <div class="flex items-center gap-3">
              <label class="text-[13px] text-ink-secondary w-[140px] shrink-0" for="cap-daily">
                Tasks per day
              </label>
              <input
                id="cap-daily"
                v-model="caps.dailyTaskCap"
                type="number"
                min="1"
                placeholder="no cap"
                class="w-[120px] bg-base border border-line rounded px-2 py-1 font-mono text-xs text-ink tabular-nums"
                @change="refresh"
              />
            </div>

            <div class="flex items-start gap-3">
              <label class="text-[13px] text-ink-secondary w-[140px] shrink-0 pt-1" for="cap-window">
                Window
              </label>
              <div class="flex flex-col gap-2">
                <label class="flex items-center gap-2 font-mono text-[11px] text-ink-tertiary uppercase tracking-[0.08em]">
                  <input id="cap-window" v-model="caps.windowEnabled" type="checkbox" />
                  only between
                </label>
                <div v-if="caps.windowEnabled" class="flex items-center gap-2">
                  <input
                    v-model="caps.windowStart"
                    type="text"
                    placeholder="00:00"
                    aria-label="Window start"
                    class="w-[68px] bg-base border border-line rounded px-2 py-1 font-mono text-xs text-ink tabular-nums"
                  />
                  <span class="font-mono text-xs text-ink-tertiary">–</span>
                  <input
                    v-model="caps.windowEnd"
                    type="text"
                    placeholder="07:00"
                    aria-label="Window end"
                    class="w-[68px] bg-base border border-line rounded px-2 py-1 font-mono text-xs text-ink tabular-nums"
                  />
                </div>
                <div
                  v-if="windowBroken"
                  class="font-mono text-[11px]"
                  style="color: var(--status-error)"
                >Both ends, as HH:MM.</div>
                <div v-else-if="caps.windowEnabled" class="text-[12px] font-light text-ink-tertiary">
                  Paces work already queued. It never starts a batch on its own.
                </div>
              </div>
            </div>

            <div class="flex items-center gap-3">
              <label class="text-[13px] text-ink-secondary w-[140px] shrink-0" for="cap-disk">
                Stop below (GB)
              </label>
              <input
                id="cap-disk"
                v-model="caps.diskFloorGb"
                type="number"
                min="1"
                placeholder="no floor"
                class="w-[120px] bg-base border border-line rounded px-2 py-1 font-mono text-xs text-ink tabular-nums"
              />
            </div>

            <div class="flex items-start gap-3">
              <label class="text-[13px] text-ink-secondary w-[140px] shrink-0 pt-1" for="cap-holds">
                Unreviewed holds
              </label>
              <div class="flex flex-col gap-1">
                <input
                  id="cap-holds"
                  v-model="caps.maxOutstandingHolds"
                  type="number"
                  min="1"
                  placeholder="no cap"
                  class="w-[120px] bg-base border border-line rounded px-2 py-1 font-mono text-xs text-ink tabular-nums"
                />
                <div v-if="preview?.has_hold" class="text-[12px] font-light text-ink-tertiary max-w-[280px]">
                  This line holds for review. Without a cap it will park every run in front
                  of you before the stages below one of them ever run.
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Save it for next time. A query plus the line you usually send it
             to — one click to repeat, and never a schedule. -->
        <details class="flex flex-col gap-2">
          <summary class="label cursor-pointer select-none hover:text-ink-secondary">
            Save this selection
          </summary>
          <div class="flex items-center gap-2 mt-2">
            <input
              v-model="saveName"
              type="text"
              placeholder="Grandma, pre-1990"
              aria-label="Selection name"
              class="flex-1 bg-base border border-line rounded px-2 py-1.5 text-[13px] text-ink"
            />
            <button
              class="border border-line rounded px-3 py-1.5 font-mono text-[11px] uppercase tracking-[0.08em] text-ink-secondary hover:bg-raised disabled:opacity-40"
              :disabled="!saveName.trim() || saving"
              @click="saveSelection"
            >{{ saved ? 'saved' : 'save' }}</button>
          </div>
          <div class="text-[12px] font-light text-ink-tertiary mt-2">
            Saved selections never run on their own. A batch exists because somebody
            pressed Send.
          </div>
        </details>

        <div v-if="preview?.refused" class="font-mono text-xs" style="color: var(--status-error)">
          {{ preview.refused }}
        </div>
        <div v-else-if="error" class="font-mono text-xs" style="color: var(--status-error)">
          {{ error }}
        </div>
      </div>

      <div class="flex items-center justify-between gap-3 px-6 py-4 border-t border-line">
        <!-- The caps, restated where the decision is made. A window and a daily
             cap set three folds up are exactly the sort of thing a person
             forgets they set. -->
        <div class="font-mono text-[11px] text-ink-tertiary uppercase tracking-[0.08em] truncate">
          <template v-if="caps.windowEnabled && !windowBroken">
            window {{ formatWindow(minutesFromTime(caps.windowStart), minutesFromTime(caps.windowEnd)) }}
          </template>
          <template v-if="caps.windowEnabled && !windowBroken && caps.dailyTaskCap"> · </template>
          <template v-if="caps.dailyTaskCap">cap {{ formatCount(Number(caps.dailyTaskCap)) }}/day</template>
        </div>
        <div class="flex items-center gap-2 shrink-0">
          <button
            class="border border-line rounded px-4 py-1.5 font-mono text-[11px] uppercase tracking-[0.08em] text-ink-secondary hover:bg-raised"
            @click="emit('close')"
          >Cancel</button>
          <button
            class="bg-signal text-signal-fg rounded px-5 py-1.5 font-mono text-[11px] uppercase tracking-[0.08em] font-medium hover:bg-signal-hover disabled:opacity-40 disabled:cursor-not-allowed"
            :disabled="!canSend"
            @click="send"
          >{{ sending ? 'sending…' : 'Send' }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

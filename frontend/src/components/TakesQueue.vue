<script setup>
/**
 * Takes lane — the contact sheet over every run parked at a hold point.
 *
 * This is the screen the whole content farm is for. Generation is cheap and
 * deciding is not: a `×4 extend → hold → upscale 4K` line makes four candidate
 * clips in minutes and then waits for somebody to say which of them is worth an
 * hour of GPU. The lane is where that hour is spent or saved, two hundred takes
 * in ten minutes, on a keyboard.
 *
 * Everything that could be wrong lives in `@/lib/takes.js` and is tested there:
 * this file listens for keys, draws what the reducer says, and performs the
 * effects it hands back. There is no second copy of the rules here.
 *
 * Three things worth knowing about the shape:
 *
 * **A verdict settles optimistically.** The run leaves the page the instant
 * Enter is pressed and the POST goes in the background, because a round trip
 * per verdict is a third of the three seconds a take is allowed. A failure puts
 * the page back and says so rather than swallowing it.
 *
 * **Rejecting is armed, not immediate.** `X` marks a take and the footer prints
 * the megabytes the next Enter will free. The bytes go with the verdict, so the
 * key costs no dialog and stays reversible right up until it is not.
 *
 * **The page refills before it runs out.** Held runs are fetched a page at a
 * time and the next page is asked for while there is still work on screen, so
 * the keyboard never waits for the network.
 */
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { useRouter } from 'vue-router'
import {
  KEY_MAP,
  backlog,
  batchNotice,
  batchOf,
  currentSheet,
  currentTake,
  formatBytes,
  initialState,
  isKept,
  isRejected,
  keyAction,
  reduce,
  settle,
  shortId,
  takeMarks,
  varyingKeys,
  verdictSummary,
} from '@/lib/takes.js'
import { continuationCost } from '@/lib/lines.js'

const emit = defineEmits(['changed'])
const router = useRouter()

const state = ref(initialState([]))
const loading = ref(true)
const error = ref('')
const cursor = ref(null)
const decided = ref(0)
const freed = ref(0)
const manifest = ref(null)
const manifestBusy = ref(false)
const playing = ref(new Set())

const PAGE = 24
/** Refill while there is still a screenful left, so the keyboard never waits. */
const REFILL_BELOW = 8

const sheet = computed(() => currentSheet(state.value))
const take = computed(() => currentTake(state.value))
const summary = computed(() => verdictSummary(state.value))
const counts = computed(() => backlog(state.value.sheets))
const varying = computed(() => varyingKeys(sheet.value?.takes || []))

/** FR7's batches, id → row. Empty whenever that endpoint cannot be asked. */
const batches = ref({})
const batch = computed(() => batchOf(sheet.value, batches.value))
const batchSays = computed(() => batchNotice(batch.value))

/** What continuing with the takes marked so far will queue below the hold. */
const cost = computed(() =>
  continuationCost(Math.max(summary.value.keep, 1), sheet.value?.tasks_per_take),
)

/**
 * The sheet fits the screen, and the screen does not scroll.
 *
 * A four-take fan-out is a 2×2 grid — the case the hold mechanism exists for —
 * and the cells divide the height they are given rather than each claiming a
 * 4:3 box and pushing the footer off the bottom. Somebody deciding in three
 * seconds must be able to see all four candidates *and* what Enter is about to
 * do, without a scroll wheel anywhere in the loop.
 *
 * Past six takes there is no honest way to show them all at once, so the grid
 * gives up and scrolls — the cursor is what keeps its place then.
 */
const gridStyle = computed(() => {
  const n = sheet.value?.takes.length || 0
  if (n <= 1) return { gridTemplateColumns: 'minmax(0, 1fr)', gridTemplateRows: 'minmax(0, 1fr)' }
  if (n === 2) {
    return { gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gridTemplateRows: 'minmax(0, 1fr)' }
  }
  if (n <= 4) {
    return { gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gridTemplateRows: 'repeat(2, minmax(0, 1fr))' }
  }
  if (n <= 6) {
    return { gridTemplateColumns: 'repeat(3, minmax(0, 1fr))', gridTemplateRows: 'repeat(2, minmax(0, 1fr))' }
  }
  return { gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))', gridAutoRows: 'minmax(160px, 1fr)' }
})

/** Past six takes the grid stops fitting and starts scrolling. */
const crowded = computed(() => (sheet.value?.takes.length || 0) > 6)

// ===== Loading =============================================================

async function fetchPage(append = false) {
  if (!append) loading.value = true
  try {
    const q = new URLSearchParams({ limit: String(PAGE) })
    if (append && cursor.value) q.set('cursor', cursor.value)
    const res = await fetch(`/api/comfyui/takes?${q}`)
    if (res.status === 503) {
      error.value = 'ComfyUI is not configured, so nothing can be generating.'
      state.value = initialState([])
      return
    }
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    error.value = ''
    cursor.value = data.next_cursor || null
    state.value = append
      ? { ...state.value, sheets: [...state.value.sheets, ...data.items] }
      : initialState(data.items)
    fetchBatches()
  } catch (e) {
    console.error('Failed to fetch held runs', e)
    error.value = String(e.message || e)
  } finally {
    loading.value = false
  }
}

function refillIfThin() {
  if (cursor.value && state.value.sheets.length < REFILL_BELOW) fetchPage(true)
}

/**
 * The names FR7 gives its batches, and whether any of them is paused.
 *
 * Asked once per page rather than per run, and **never allowed to fail loudly**:
 * this endpoint is FR7's and may not be there at all, in which case every run
 * still draws with the batch id it already had. A lane that could not render a
 * held run because a name was unavailable would be trading the thing it is for
 * a decoration.
 *
 * Re-asked after a verdict, because clearing a run is exactly what lifts a
 * hold-cap pause — the sentence under the tag should stop being true on the
 * screen of the person who made it stop being true.
 */
async function fetchBatches() {
  if (!state.value.sheets.some((s) => s.batch_id)) return
  try {
    const res = await fetch('/api/comfyui/batches')
    if (!res.ok) return
    const data = await res.json()
    const rows = Array.isArray(data) ? data : data.items || []
    batches.value = Object.fromEntries(rows.filter((r) => r?.id).map((r) => [r.id, r]))
  } catch {
    // No names today. Every run still draws.
  }
}

// ===== Keys ================================================================

function onKeydown(e) {
  const action = keyAction(e, state.value)
  if (!action) return
  e.preventDefault()
  const result = reduce(state.value, action)
  state.value = result.state
  perform(result.effects)
}

async function perform(effects) {
  for (const fx of effects) {
    if (fx.kind === 'verdict') sendVerdict(fx)
    else if (fx.kind === 'rate') await rate(fx)
    else if (fx.kind === 'promote') await promote(fx)
    else if (fx.kind === 'provenance') await readManifest(fx)
    else if (fx.kind === 'play') togglePlay(fx.taskId)
  }
}

/** Dispatch an action the way a click does, so mouse and keyboard agree. */
function act(action) {
  const result = reduce(state.value, action)
  state.value = result.state
  perform(result.effects)
}

// ===== Effects =============================================================

/**
 * Send the verdict, having already taken the run off the page.
 *
 * The optimistic settle is what makes the lane fast; the reload on failure is
 * what makes it honest. Nothing is guessed about what the runtime did — a
 * verdict that did not land puts the whole page back from the server.
 */
async function sendVerdict(fx) {
  const before = state.value
  state.value = settle(state.value, [fx.runId])
  refillIfThin()
  try {
    const res = await fetch(`/api/comfyui/runs/${fx.runId}/hold`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        verdict: fx.verdict,
        keep: fx.keep,
        reject: fx.reject,
        scope: fx.scope,
      }),
    })
    const data = await res.json().catch(() => ({}))
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)

    decided.value += 1 + (data.also_applied?.length || 0)
    freed.value += Number(data.freed_bytes) || 0
    // A bulk verdict took siblings with it; they are not this page's business
    // any more either.
    if (data.also_applied?.length) {
      state.value = settle(state.value, data.also_applied)
    }
    if (data.failed?.length) {
      state.value = {
        ...state.value,
        said: `${data.failed.length} run(s) of the batch could not take that verdict.`,
      }
    }
    emit('changed')
    refillIfThin()
    if (batch.value?.paused) fetchBatches()
  } catch (e) {
    console.error('Verdict failed', e)
    state.value = { ...before, said: `That verdict did not land: ${e.message}` }
    error.value = ''
    fetchPage(false)
  }
}

async function rate(fx) {
  try {
    await fetch(`/api/files/${fx.fileId}/rating`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ rating: fx.rating }),
    })
  } catch (e) {
    // A rating is a note to self, not a decision. Losing one to a dropped
    // connection is not worth interrupting a review over.
    console.warn('Could not store rating', e)
  }
}

async function promote(fx) {
  try {
    const res = await fetch(`/api/files/${fx.fileId}/set-original`, { method: 'PUT' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    if (sheet.value) {
      for (const t of sheet.value.takes) t.is_main_file = t.output_file_id === fx.fileId
      sheet.value.main_file_id = fx.fileId
    }
    state.value = { ...state.value, said: 'Promoted to the shot’s main file.' }
    emit('changed')
  } catch (e) {
    console.error('Promote failed', e)
    state.value = { ...state.value, said: `Could not promote: ${e.message}` }
  }
}

async function readManifest(fx) {
  manifest.value = null
  manifestBusy.value = true
  try {
    const res = await fetch(`/api/files/${fx.fileId}/manifest`)
    if (res.ok) manifest.value = await res.json()
  } catch (e) {
    console.warn('Could not read provenance', e)
  } finally {
    manifestBusy.value = false
  }
}

function togglePlay(taskId) {
  const el = document.querySelector(`[data-take="${CSS.escape(taskId)}"] video`)
  if (!el) return
  const next = new Set(playing.value)
  if (el.paused) {
    el.play().catch(() => {})
    next.add(taskId)
  } else {
    el.pause()
    next.delete(taskId)
  }
  playing.value = next
}

// ===== Drawing =============================================================

const isVideo = (t) => String(t?.mime_type || '').startsWith('video/')

function ratingOf(t) {
  const local = state.value.ratings[t.task_id]
  return local === undefined ? t.rating : local
}

function cardClass(t, i) {
  const focused = i === state.value.take
  if (isRejected(state.value, sheet.value.run_id, t.task_id)) {
    return focused ? 'border-error opacity-60' : 'border-error opacity-40'
  }
  if (isKept(state.value, sheet.value.run_id, t.task_id)) return 'border-signal'
  return focused ? 'border-signal' : 'border-line'
}

/** Keep the focused card and the current run on screen as the cursor moves. */
watch(
  () => [state.value.run, state.value.take],
  async () => {
    await nextTick()
    document
      .querySelector('[data-strip-current="1"]')
      ?.scrollIntoView({ block: 'nearest', inline: 'center' })
  },
)

// The provenance panel is about one take, so moving off it closes it rather
// than quietly showing another take's manifest.
watch(
  () => take.value?.task_id,
  () => {
    if (state.value.provenance) state.value = { ...state.value, provenance: false }
    manifest.value = null
  },
)

onMounted(() => {
  fetchPage(false)
  window.addEventListener('keydown', onKeydown)
})
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

defineExpose({ loadData: () => fetchPage(false) })
</script>

<template>
  <div class="flex flex-col flex-1 min-h-0">
    <!-- Loading / empty / error: the three states every lane here draws. -->
    <div v-if="loading" class="flex-1 flex items-center justify-center py-16">
      <span class="font-mono text-xs text-ink-tertiary">
        reading held runs <span class="text-building signal-pulse">●</span>
      </span>
    </div>

    <div
      v-else-if="error"
      class="flex-1 flex flex-col items-center justify-center gap-2 p-16 text-center"
    >
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-error)"></span>
      <div class="font-heading text-base font-semibold text-ink">Could not read the backlog</div>
      <div class="font-mono text-xs text-error">{{ error }}</div>
      <button
        class="mt-2 border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
        @click="fetchPage(false)"
      >Retry</button>
    </div>

    <div
      v-else-if="!sheet"
      class="flex-1 flex flex-col items-center justify-center gap-2 p-16 text-center"
    >
      <span class="signal-dot" style="width:10px;height:10px;background:var(--status-ready)"></span>
      <div class="font-heading text-base font-semibold text-ink">Nothing waiting on a verdict</div>
      <div class="text-[13px] font-light text-ink-secondary max-w-md">
        A stage marked <span class="font-mono">hold for review</span> parks its run here with its
        takes. Until one does, the queue is running unattended.
      </div>
      <div v-if="decided" class="label mt-2">
        {{ decided }} decided this session<template v-if="freed"> · {{ formatBytes(freed) }} freed</template>
      </div>
    </div>

    <!-- The lane proper: header, stage, strip, footer, and no scroll bar
         anywhere in the loop. -->
    <template v-else>
      <div
        data-lane="takes"
        class="flex-1 min-h-0 overflow-hidden flex flex-col px-4 md:px-8 pt-4 pb-3 gap-3"
      >
          <!-- Which run, out of how many, and where in its line. -->
          <div class="flex-none flex items-baseline justify-between gap-4 flex-wrap">
            <div class="flex items-baseline gap-3 min-w-0">
              <span class="label">shot</span>
              <span class="font-mono text-[13px] text-ink truncate">{{ shortId(sheet.shot_id) }}</span>
              <span class="text-ink-tertiary">·</span>
              <span class="font-mono text-[13px] uppercase tracking-[0.08em] text-ink truncate">
                {{ sheet.stage_label || sheet.label }}
              </span>
              <span class="label">stage {{ sheet.stage_idx + 1 }} / {{ sheet.stage_count }}</span>
            </div>
            <div class="font-mono text-xs text-ink-tertiary whitespace-nowrap">
              run {{ state.run + 1 }} / {{ counts.runs }} · {{ counts.takes }} takes waiting
            </div>
          </div>

          <div class="flex-none flex items-center gap-2 flex-wrap">
            <span
              class="tag bg-base"
              style="color: var(--status-degraded); border-color: var(--status-degraded)"
            >held</span>
            <span
              v-if="batch"
              class="tag bg-base"
              :class="batch.paused ? '' : 'text-ink-tertiary'"
              :style="batch.paused
                ? 'color: var(--status-degraded); border-color: var(--status-degraded)'
                : ''"
              :title="batch.named ? batch.id : undefined"
            >batch {{ batch.label }}</span>
            <span class="label">
              keeping one costs {{ cost }} task<template v-if="cost !== 1">s</template> below
            </span>
            <span
              v-if="batchSays"
              class="font-mono text-[11px]"
              style="color: var(--status-degraded)"
            >{{ batchSays }}</span>
            <span class="flex-1"></span>
            <button class="label hover:text-signal transition-colors" @click="act({ type: 'help' })">
              <kbd class="kbd-ab">?</kbd> keys
            </button>
          </div>

          <!-- Compare: the original the takes are variations of, beside them. -->
          <div class="flex-1 min-h-0 flex gap-4 items-stretch">
            <div
              v-if="sheet.source_thumbnail_url"
              class="flex-none flex flex-col gap-1 min-h-0"
              :class="state.compare ? 'w-[38%] max-w-[560px]' : 'w-[124px]'"
            >
              <div
                class="relative bg-surface border border-line rounded overflow-hidden"
                :class="state.compare ? 'flex-1 min-h-0' : 'aspect-[4/3] flex-none'"
              >
                <img
                  :src="sheet.source_thumbnail_url"
                  class="absolute inset-0 w-full h-full object-contain"
                  loading="lazy"
                />
              </div>
              <div class="flex-none flex items-center justify-between gap-2">
                <span class="label">original</span>
                <button
                  class="font-mono text-[10px] text-ink-tertiary hover:text-signal transition-colors"
                  @click="act({ type: 'compare' })"
                >
                  {{ state.compare ? 'shrink' : 'compare' }} <kbd class="kbd-ab">C</kbd>
                </button>
              </div>
            </div>

            <!-- A run marked held with nothing left to look at. Every take it
                 made already carries a verdict, so there is nothing to choose
                 between — but the run is still parked, and somebody has to be
                 able to say what happens to it. -->
            <div
              v-if="!sheet.takes.length"
              class="flex-1 min-w-0 min-h-0 flex flex-col items-center justify-center gap-2 text-center border border-line rounded"
            >
              <span class="signal-dot" style="width:8px;height:8px;background:var(--status-degraded)"></span>
              <div class="font-heading text-[15px] font-semibold text-ink">Nothing left to look at</div>
              <div class="text-[13px] font-light text-ink-secondary max-w-sm">
                This run is still held, but every take it produced already has a verdict.
                Regenerate for a fresh set, or abandon the run.
              </div>
            </div>

            <div
              v-else
              class="flex-1 min-w-0 min-h-0 grid gap-3 w-full"
              :class="crowded ? 'overflow-y-auto' : ''"
              :style="gridStyle"
            >
              <div
                v-for="(t, i) in sheet.takes"
                :key="t.task_id"
                :data-take="t.task_id"
                :data-focused="i === state.take ? '1' : '0'"
                class="flex flex-col min-h-0"
              >
                <button
                  class="flex-1 min-h-0 bg-surface border rounded overflow-hidden relative p-0 flex items-center justify-center transition-opacity w-full"
                  :class="cardClass(t, i)"
                  @click="state = { ...state, take: i }"
                  @dblclick="act({ type: 'keep', commit: true })"
                >
                  <!-- The cursor: a signal rule across the top of the focused
                       card, which is how a schedule board points at a row. -->
                  <span
                    v-if="i === state.take"
                    class="absolute top-0 left-0 right-0 h-[3px] bg-signal z-10"
                  ></span>

                  <video
                    v-if="isVideo(t) && t.file_url"
                    :src="t.file_url"
                    :poster="t.thumbnail_url || undefined"
                    class="absolute inset-0 w-full h-full object-contain bg-raised"
                    muted
                    loop
                    playsinline
                    preload="metadata"
                  ></video>
                  <img
                    v-else-if="t.thumbnail_url"
                    :src="t.thumbnail_url"
                    class="absolute inset-0 w-full h-full object-contain bg-raised"
                    loading="lazy"
                  />
                  <span
                    v-else
                    class="absolute inset-0 p-3 text-left font-mono text-[12px] leading-relaxed text-ink-secondary overflow-hidden bg-raised"
                  >{{ t.text_output || '—' }}</span>

                  <span
                    v-if="isVideo(t)"
                    class="absolute bottom-2 right-2 z-10 font-mono text-[10px] text-ink-tertiary bg-base border border-line rounded-sm px-1"
                  >{{ playing.has(t.task_id) ? '❚❚' : '▶' }}</span>

                  <span
                    v-if="isRejected(state, sheet.run_id, t.task_id)"
                    class="tag absolute top-2 left-2 bg-base z-10"
                    style="color: var(--status-error); border-color: var(--status-error)"
                  >reject</span>
                  <span
                    v-else-if="isKept(state, sheet.run_id, t.task_id)"
                    class="tag absolute top-2 left-2 bg-base z-10"
                    style="color: var(--accent); border-color: var(--accent-muted)"
                  >keep</span>
                  <span
                    v-if="t.is_main_file"
                    class="tag absolute top-2 right-2 bg-base text-ink-tertiary z-10"
                  >main</span>
                </button>

                <!-- The strip: what makes this take this one, and its rating. -->
                <div class="flex items-center justify-between gap-2 px-1 pt-1.5">
                  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-tertiary truncate">
                    <template v-for="(mark, m) in takeMarks(t, varying, i)" :key="m">
                      <span v-if="m"> · </span>{{ mark.label }} {{ mark.value }}
                    </template>
                  </span>
                  <span class="flex gap-[2px] flex-none">
                    <span
                      v-for="n in 5"
                      :key="n"
                      class="w-[5px] h-[10px] border rounded-[1px]"
                      :style="{
                        borderColor: (ratingOf(t) || 0) >= n ? 'var(--accent)' : 'var(--border-strong)',
                        background: (ratingOf(t) || 0) >= n ? 'var(--accent)' : 'transparent',
                      }"
                    ></span>
                  </span>
                </div>
                <div v-if="t.file_size" class="px-1 font-mono text-[10px] text-ink-tertiary">
                  {{ formatBytes(t.file_size) }}
                </div>
              </div>
            </div>

            <!-- How this take was made, and how to make another. -->
            <div
              v-if="state.provenance"
              class="flex-none w-[300px] border-l border-line pl-4 flex flex-col gap-3 overflow-y-auto"
            >
            <div class="flex items-baseline justify-between gap-4">
              <span class="label">provenance</span>
              <button
                class="font-mono text-[10px] text-ink-tertiary hover:text-signal transition-colors"
                @click="act({ type: 'provenance' })"
              >close <kbd class="kbd-ab">esc</kbd></button>
            </div>
            <div v-if="manifestBusy" class="font-mono text-xs text-ink-tertiary">reading…</div>
            <div v-else-if="!manifest?.manifest" class="font-mono text-xs text-ink-tertiary">
              No manifest was written for this file.
            </div>
            <dl v-else class="grid gap-x-6 gap-y-1.5" style="grid-template-columns: max-content 1fr">
              <template
                v-for="row in [
                  ['line', manifest.manifest.line_id],
                  ['stage', manifest.manifest.stage_index !== null && manifest.manifest.stage_index !== undefined ? manifest.manifest.stage_index + 1 : null],
                  ['workflow', manifest.manifest.workflow_id],
                  ['parent file', manifest.manifest.source_file_id],
                  ['seed', manifest.manifest.seed],
                  ['prompt id', manifest.manifest.comfyui_prompt_id],
                  ['made', manifest.manifest.generated_at],
                ]"
                :key="row[0]"
              >
                <dt v-if="row[1] !== null && row[1] !== undefined" class="label self-center">{{ row[0] }}</dt>
                <dd
                  v-if="row[1] !== null && row[1] !== undefined"
                  class="font-mono text-[11px] text-ink-secondary break-all"
                >{{ row[1] }}</dd>
              </template>
            </dl>
            <div
              v-if="manifest?.manifest?.parameters && Object.keys(manifest.manifest.parameters).length"
              class="font-mono text-[10px] text-ink-tertiary break-all leading-relaxed"
            >
              <template v-for="(v, k) in manifest.manifest.parameters" :key="k">
                {{ k }}={{ v }}&nbsp;&nbsp;
              </template>
            </div>
            <div class="flex flex-wrap gap-2 border-t border-line pt-3">
              <button
                class="border border-line-strong rounded px-3 py-1.5 text-[13px] text-ink-secondary hover:text-signal transition-colors"
                @click="act({ type: 'regenerate' })"
              >Make another like this <kbd class="ml-1 kbd-ab">R</kbd></button>
              <button
                class="border border-line-strong rounded px-3 py-1.5 text-[13px] text-ink-secondary hover:text-signal transition-colors"
                @click="router.push(`/shot/${sheet.shot_id}`)"
              >Edit and re-run</button>
            </div>
          </div>
          </div>

          <!-- The rest of the backlog, so ↑ ↓ has somewhere visible to go. -->
          <div v-if="state.sheets.length > 1" class="flex-none flex items-center gap-3 border-t border-line pt-2.5">
            <span class="label flex-none">waiting</span>
            <div class="flex gap-2 overflow-x-auto pb-0.5">
              <button
                v-for="(s, i) in state.sheets"
                :key="s.run_id"
                :data-strip-current="i === state.run ? '1' : '0'"
                class="w-14 h-10 flex-none rounded-sm bg-raised border overflow-hidden relative p-0"
                :class="i === state.run ? 'border-signal' : 'border-line'"
                :title="`${s.stage_label || s.label} — ${s.takes.length} takes`"
                @click="state = { ...state, run: i, take: 0 }"
              >
                <img
                  v-if="s.takes[0]?.thumbnail_url"
                  :src="s.takes[0].thumbnail_url"
                  class="w-full h-full object-cover"
                  loading="lazy"
                />
                <span
                  class="absolute bottom-0 right-0 font-mono text-[9px] px-1 bg-base text-ink-tertiary"
                >{{ s.takes.length }}</span>
              </button>
            </div>
          </div>
      </div>

      <!-- The footer is the safeguard: what the next Enter does, always on
           screen, including the megabytes it frees. -->
      <div class="border-t border-line px-4 md:px-8 py-3 flex flex-col gap-2 flex-none bg-base">
        <div v-if="state.help" class="card-ab p-4 grid gap-x-8 gap-y-1" style="grid-template-columns: repeat(auto-fill, minmax(280px, 1fr))">
          <div v-for="row in KEY_MAP" :key="row.keys" class="flex items-baseline gap-3">
            <kbd class="kbd-ab flex-none">{{ row.keys }}</kbd>
            <span class="text-[12px] font-light text-ink-secondary">{{ row.does }}</span>
          </div>
        </div>

        <div v-if="state.said" class="font-mono text-[11px]" :class="state.armed ? 'text-error' : 'text-ink-tertiary'">
          {{ state.said }}
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <button
            class="rounded px-4 py-2 text-[13px] font-medium transition-colors"
            :class="sheet.takes.length
              ? 'bg-signal text-signal-fg hover:bg-signal-hover'
              : 'border border-line text-ink-tertiary cursor-not-allowed'"
            :disabled="!sheet.takes.length"
            @click="act({ type: 'keep', commit: true })"
          >
            Keep this take
            <kbd class="ml-1 font-mono text-[10px] border rounded-sm px-1" style="border-color: oklch(15% 0.01 80 / .3)">⏎</kbd>
          </button>
          <button
            class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
            @click="act({ type: 'reject' })"
          >Reject <kbd class="ml-1 kbd-ab">X</kbd></button>
          <button
            class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
            @click="act({ type: 'regenerate' })"
          >Regenerate <kbd class="ml-1 kbd-ab">R</kbd></button>
          <button
            class="border rounded px-4 py-2 text-[13px] transition-colors"
            :class="state.armed === 'cancel'
              ? 'border-error text-error'
              : 'border-line-strong text-ink-secondary hover:text-signal'"
            @click="act({ type: 'cancel' })"
          >
            {{ state.armed === 'cancel' ? 'Abandon — press again' : 'Abandon' }}
            <kbd class="ml-1 kbd-ab">⌫</kbd>
          </button>
          <button
            v-if="sheet.batch_id"
            class="border rounded px-4 py-2 text-[13px] transition-colors"
            :class="state.bulk ? 'border-signal text-signal' : 'border-line-strong text-ink-secondary hover:text-signal'"
            @click="act({ type: 'bulk' })"
          >
            {{ state.bulk ? 'Next verdict → whole batch' : 'Apply to whole batch' }}
            <kbd class="ml-1 kbd-ab">B</kbd>
          </button>

          <span class="flex-1"></span>

          <!-- The three numbers the next keystroke turns into actions. -->
          <span class="font-mono text-[11px] uppercase tracking-[0.08em] whitespace-nowrap">
            <span class="text-signal">keep {{ summary.keep }}</span>
            <span class="text-ink-tertiary"> · </span>
            <span :class="summary.reject ? 'text-error' : 'text-ink-tertiary'">
              reject {{ summary.reject }}<template v-if="summary.bytes"> ({{ formatBytes(summary.bytes) }})</template>
            </span>
            <span class="text-ink-tertiary"> · pass {{ summary.pass }}</span>
          </span>
        </div>

        <div class="flex items-baseline gap-3 flex-wrap">
          <span class="font-mono text-[10px] text-ink-tertiary">
            rejecting deletes the file. Nothing goes until the verdict is sent.
          </span>
          <span class="flex-1"></span>
          <span v-if="decided" class="font-mono text-[10px] text-ink-tertiary">
            {{ decided }} decided this session<template v-if="freed"> · {{ formatBytes(freed) }} freed</template>
          </span>
        </div>
      </div>
    </template>
  </div>
</template>

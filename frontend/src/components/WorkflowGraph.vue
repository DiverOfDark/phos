<script setup>
/**
 * A ComfyUI workflow as a route diagram.
 *
 * ComfyUI's own editor draws a free-form canvas the author arranged by hand; the
 * API-format JSON Phos stores keeps none of that geometry, only nodes and the
 * links between them. So the layout is derived: rank each node by how far it is
 * from a source, stack the ranks left to right, and run the links as tracks
 * through the gap in front of their target — the same diagram the Overview uses
 * for the library pipeline, because it is the same kind of fact.
 */
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'

const props = defineProps({
  workflowId: { type: String, required: true },
  /** Node ids the Enhance dialog can override, so they can be marked as editable. */
  editableNodeIds: { type: Array, default: () => [] },
})

const NODE_W = 184
/** Compact nodes carry only a type and an id, so they need less room. */
const NODE_W_COMPACT = 168
const HEADER_H = 26
const ROW_H = 15
const PAD_Y = 8
const MAX_ROWS = 5
const COL_GAP = 76
const COL_GAP_COMPACT = 56
const ROW_GAP = 20
const MARGIN = 16

const graph = ref(null)
const loading = ref(false)
const error = ref('')
const selectedId = ref(null)
const hoverId = ref(null)
const showJson = ref(false)
/** Header-only nodes. A 40-node graph is a shape to read, not a table to study. */
const compact = ref(false)

const nodeW = computed(() => (compact.value ? NODE_W_COMPACT : NODE_W))
const colGap = computed(() => (compact.value ? COL_GAP_COMPACT : COL_GAP))

/** The scroller, so the header can say when there is more graph off to the right. */
const scroller = ref(null)
const overflows = ref(false)

function measure() {
  const el = scroller.value
  overflows.value = !!el && el.scrollWidth > el.clientWidth + 1
}

onMounted(() => {
  measure()
  window.addEventListener('resize', measure)
})
onUnmounted(() => window.removeEventListener('resize', measure))

async function load() {
  if (!props.workflowId) return
  loading.value = true
  error.value = ''
  selectedId.value = null
  try {
    const res = await fetch(`/api/comfyui/workflows/${props.workflowId}/graph`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    graph.value = await res.json()
  } catch (e) {
    error.value = e.message || 'could not load the graph'
    graph.value = null
  } finally {
    loading.value = false
  }
}

watch(() => props.workflowId, load, { immediate: true })

/** A link is any input whose value is a `[nodeId, slot]` pair. */
function isLink(value) {
  return Array.isArray(value) && value.length === 2 && typeof value[0] === 'string'
}

function shortValue(value) {
  if (value === null || value === undefined) return ''
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return Array.isArray(value) ? '[…]' : '{…}'
}

/**
 * A node row is one line of 10px mono, so it holds about 30 characters. The
 * label gets what it needs up to half of that and the value takes the rest —
 * without a shared budget a long checkpoint filename runs straight through its
 * own field name.
 */
const ROW_CHARS = 25
/** JetBrains Mono is ~0.6em wide, so 11px type is ~6.6px a character. */
const CHAR_W = 6.6
function fitRow(label, value) {
  const cap = (text, n) => (text.length > n ? text.slice(0, Math.max(1, n - 1)) + '…' : text)
  if (!value) return { label: cap(label, ROW_CHARS), value: '' }
  const labelText = cap(label, Math.max(8, ROW_CHARS - Math.min(value.length, 18) - 1))
  return { label: labelText, value: cap(value, ROW_CHARS - labelText.length - 1) }
}

/** Nodes that end the graph — what the workflow is actually for. */
const OUTPUT_TYPES = new Set(['SaveImage', 'PreviewImage', 'SaveAnimatedWEBP', 'SaveAnimatedPNG', 'VHS_VideoCombine'])

const model = computed(() => {
  const raw = graph.value?.graph
  if (!raw || typeof raw !== 'object') return null

  const editable = new Set(props.editableNodeIds)
  const nodes = new Map()

  for (const [id, node] of Object.entries(raw)) {
    if (!node || typeof node !== 'object') continue
    const inputs = node.inputs && typeof node.inputs === 'object' ? node.inputs : {}
    const fields = []
    const links = []
    for (const [field, value] of Object.entries(inputs)) {
      if (isLink(value)) {
        links.push({ field, from: String(value[0]), slot: value[1] })
        fields.push({ field, linked: true, label: field, value: '', full: '' })
      } else {
        const full = shortValue(value)
        const row = fitRow(field, full)
        fields.push({ field, linked: false, label: row.label, value: row.value, full })
      }
    }
    const type = node.class_type || 'Unknown'
    // The header is a left-anchored type and a right-anchored id sharing one
    // line; without a budget "UpscaleModelLoader" runs straight through "149".
    const typeRoom = Math.floor((nodeW.value - 32 - (String(id).length * CHAR_W + 10)) / CHAR_W)
    nodes.set(id, {
      id,
      type,
      typeLabel: type.length > typeRoom ? type.slice(0, Math.max(3, typeRoom - 1)) + '…' : type,
      title: node._meta?.title || null,
      fields,
      links,
      editable: editable.has(id),
      hasOutgoing: false,
    })
  }

  if (nodes.size === 0) return null

  // Links to nodes that are not in the graph are dropped rather than drawn into
  // nowhere — a hand-edited export can reference a node its author deleted.
  for (const node of nodes.values()) {
    node.links = node.links.filter((l) => nodes.has(l.from))
    for (const l of node.links) nodes.get(l.from).hasOutgoing = true
  }

  for (const node of nodes.values()) {
    node.role = node.links.length === 0
      ? 'source'
      : (!node.hasOutgoing || OUTPUT_TYPES.has(node.type)) ? 'output' : 'step'
  }

  // --- Rank: longest path from a source. Iterative, with a cap, so a graph that
  // --- somehow contains a cycle settles instead of spinning.
  const rank = new Map()
  for (const id of nodes.keys()) rank.set(id, 0)
  for (let pass = 0; pass < nodes.size + 1; pass++) {
    let changed = false
    for (const node of nodes.values()) {
      for (const l of node.links) {
        const candidate = rank.get(l.from) + 1
        if (candidate > rank.get(node.id)) {
          rank.set(node.id, candidate)
          changed = true
        }
      }
    }
    if (!changed) break
  }

  const columns = []
  for (const node of nodes.values()) {
    const r = rank.get(node.id)
    ;(columns[r] ||= []).push(node)
  }

  for (const node of nodes.values()) {
    const rows = Math.min(node.fields.length, MAX_ROWS) + (node.fields.length > MAX_ROWS ? 1 : 0)
    node.h = compact.value
      ? HEADER_H + 8
      : HEADER_H + PAD_Y * 2 + Math.max(rows, 1) * ROW_H
    node.x = MARGIN + rank.get(node.id) * (nodeW.value + colGap.value)
  }

  // --- Order within each column by the average height of what feeds it, which is
  // --- the cheap way to stop tracks crossing each other for no reason.
  const place = () => {
    let tallest = 0
    for (const col of columns) {
      if (!col) continue
      const height = col.reduce((sum, n) => sum + n.h, 0) + (col.length - 1) * ROW_GAP
      tallest = Math.max(tallest, height)
    }
    for (const col of columns) {
      if (!col) continue
      const height = col.reduce((sum, n) => sum + n.h, 0) + (col.length - 1) * ROW_GAP
      let y = MARGIN + (tallest - height) / 2
      for (const node of col) {
        node.y = y
        y += node.h + ROW_GAP
      }
    }
    return tallest
  }

  let tallest = place()
  for (let pass = 0; pass < 2; pass++) {
    for (const col of columns) {
      if (!col || col.length < 2) continue
      for (const node of col) {
        const parents = node.links.map((l) => nodes.get(l.from)).filter(Boolean)
        node.bary = parents.length
          ? parents.reduce((sum, p) => sum + p.y + p.h / 2, 0) / parents.length
          : node.y + node.h / 2
      }
      col.sort((a, b) => a.bary - b.bary)
    }
    tallest = place()
  }

  // --- Tracks. The vertical leg sits in the gap immediately before the target,
  // --- so an edge that skips columns still turns where the reader expects it to.
  const edges = []
  for (const node of nodes.values()) {
    node.links.forEach((l) => {
      const from = nodes.get(l.from)
      // In detail mode an edge lands on the exact field it feeds; in compact mode
      // there are no rows to land on, so it meets the node's edge at its middle.
      const rowIndex = node.fields.findIndex((f) => f.field === l.field)
      const capped = Math.min(rowIndex < 0 ? 0 : rowIndex, MAX_ROWS - 1)
      const x1 = from.x + nodeW.value
      const y1 = from.y + from.h / 2
      const x2 = node.x
      const y2 = compact.value
        ? node.y + node.h / 2
        : node.y + HEADER_H + PAD_Y + capped * ROW_H + ROW_H / 2
      const midX = x2 - colGap.value / 2
      edges.push({
        key: `${l.from}:${l.slot}->${node.id}.${l.field}`,
        from: l.from,
        to: node.id,
        d: trackPath(x1, y1, midX, x2, y2),
        x2,
        y2,
      })
    })
  }

  const width = MARGIN * 2 + (columns.length - 1) * (nodeW.value + colGap.value) + nodeW.value
  const height = tallest + MARGIN * 2

  return { nodes: [...nodes.values()], edges, width, height, columns: columns.length }
})

/** An orthogonal run with 8px corners — a track, not a wire. */
function trackPath(x1, y1, midX, x2, y2) {
  if (Math.abs(y1 - y2) < 1) return `M ${x1} ${y1} L ${x2} ${y2}`
  const r = Math.min(8, Math.abs(y2 - y1) / 2, Math.abs(midX - x1), Math.abs(x2 - midX))
  const down = y2 > y1 ? 1 : -1
  return [
    `M ${x1} ${y1}`,
    `L ${midX - r} ${y1}`,
    `Q ${midX} ${y1} ${midX} ${y1 + r * down}`,
    `L ${midX} ${y2 - r * down}`,
    `Q ${midX} ${y2} ${midX + r} ${y2}`,
    `L ${x2} ${y2}`,
  ].join(' ')
}

watch(model, () => nextTick(measure))

const active = computed(() => hoverId.value || selectedId.value)

function edgeColor(edge) {
  if (active.value && (edge.from === active.value || edge.to === active.value)) return 'var(--accent)'
  return 'var(--border-strong)'
}

function roleColor(node) {
  if (node.role === 'source') return 'var(--status-pending)'
  if (node.role === 'output') return 'var(--status-ready)'
  if (node.editable) return 'var(--accent)'
  return 'var(--text-tertiary)'
}

function borderColor(node) {
  if (active.value === node.id) return 'var(--accent)'
  if (node.editable) return 'var(--accent-muted)'
  return 'var(--border)'
}

const selected = computed(() => model.value?.nodes.find((n) => n.id === selectedId.value) || null)

/** The nodes feeding the selected one, named so the panel can say where a value came from. */
function sourceLabel(link) {
  const from = model.value?.nodes.find((n) => n.id === link.from)
  return from ? `${from.id} · ${from.type}` : link.from
}
</script>

<template>
  <div class="flex flex-col gap-2 min-w-0">
    <div class="flex items-baseline justify-between gap-4">
      <div class="label">Graph</div>
      <div class="flex items-center gap-4">
        <div v-if="model" class="font-mono text-[11px] text-ink-tertiary">
          {{ model.nodes.length }} nodes · {{ model.edges.length }} links · {{ model.columns }} stages<span
            v-if="overflows"
          > · scrolls →</span>
        </div>
        <button
          class="font-mono text-[11px] transition-colors"
          :class="compact ? 'text-signal' : 'text-ink-tertiary hover:text-signal'"
          @click="compact = !compact"
        >compact</button>
        <button
          class="font-mono text-[11px] text-ink-tertiary hover:text-signal transition-colors"
          @click="showJson = !showJson"
        >{{ showJson ? 'hide json' : 'show json' }}</button>
      </div>
    </div>

    <div v-if="loading" class="card-ab p-6 font-mono text-xs text-ink-tertiary">loading graph…</div>
    <div v-else-if="error" class="card-ab p-6 font-mono text-xs text-error">{{ error }}</div>
    <div v-else-if="!model" class="card-ab p-6 font-mono text-xs text-ink-tertiary">
      this workflow has no readable nodes
    </div>

    <template v-else>
      <div ref="scroller" class="card-ab overflow-x-auto">
        <svg
          :viewBox="`0 0 ${model.width} ${model.height}`"
          :width="model.width"
          :height="model.height"
          class="block max-w-none"
          @click="selectedId = null"
        >
          <!-- Tracks first, so a node always sits on top of the lines feeding it. -->
          <g fill="none" stroke-width="1.5">
            <path
              v-for="e in model.edges"
              :key="e.key"
              :d="e.d"
              :stroke="edgeColor(e)"
            />
          </g>
          <!-- The arrowhead is a solid dot at the field it lands on. -->
          <circle
            v-for="e in model.edges"
            :key="`dot-${e.key}`"
            :cx="e.x2"
            :cy="e.y2"
            r="2.5"
            :fill="edgeColor(e)"
          />

          <g
            v-for="n in model.nodes"
            :key="n.id"
            style="cursor: pointer"
            @mouseenter="hoverId = n.id"
            @mouseleave="hoverId = null"
            @click.stop="selectedId = selectedId === n.id ? null : n.id"
          >
            <rect
              :x="n.x"
              :y="n.y"
              :width="nodeW"
              :height="n.h"
              rx="4"
              fill="var(--bg-surface)"
              :stroke="borderColor(n)"
              stroke-width="1"
            />
            <!-- Signal light: where the graph starts, what it produces, what you can edit. -->
            <circle :cx="n.x + 12" :cy="n.y + 13" r="3.5" :fill="roleColor(n)" />
            <text
              :x="n.x + 24"
              :y="n.y + 17"
              font-family="var(--font-mono)"
              font-size="11"
              font-weight="500"
              fill="var(--text-primary)"
            >{{ n.typeLabel }}</text>
            <text
              :x="n.x + nodeW - 8"
              :y="n.y + 17"
              text-anchor="end"
              font-family="var(--font-mono)"
              font-size="10"
              fill="var(--text-tertiary)"
            >{{ n.id }}</text>
            <line
              v-if="!compact"
              :x1="n.x"
              :y1="n.y + HEADER_H"
              :x2="n.x + nodeW"
              :y2="n.y + HEADER_H"
              stroke="var(--border)"
              stroke-width="1"
            />
            <template v-if="!compact">
            <template v-for="(f, i) in n.fields.slice(0, MAX_ROWS)" :key="f.field">
              <text
                :x="n.x + 10"
                :y="n.y + HEADER_H + PAD_Y + i * ROW_H + 11"
                font-family="var(--font-mono)"
                font-size="10"
                :fill="f.linked ? 'var(--text-secondary)' : 'var(--text-tertiary)'"
              >{{ f.label }}</text>
              <text
                v-if="!f.linked && f.value"
                :x="n.x + nodeW - 10"
                :y="n.y + HEADER_H + PAD_Y + i * ROW_H + 11"
                text-anchor="end"
                font-family="var(--font-mono)"
                font-size="10"
                fill="var(--text-secondary)"
              >{{ f.value }}</text>
            </template>
            </template>
            <text
              v-if="!compact && n.fields.length > MAX_ROWS"
              :x="n.x + 10"
              :y="n.y + HEADER_H + PAD_Y + MAX_ROWS * ROW_H + 11"
              font-family="var(--font-mono)"
              font-size="10"
              fill="var(--text-tertiary)"
            >+{{ n.fields.length - MAX_ROWS }} more</text>
          </g>
        </svg>
      </div>

      <!-- Legend -->
      <div class="flex flex-wrap items-center gap-4 font-mono text-[11px] text-ink-tertiary">
        <span class="flex items-center gap-1.5">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--status-pending)"></span>source
        </span>
        <span class="flex items-center gap-1.5">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--accent)"></span>editable input
        </span>
        <span class="flex items-center gap-1.5">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--status-ready)"></span>output
        </span>
        <span class="flex-1"></span>
        <span>click a node for its values</span>
      </div>

      <!-- Selected node -->
      <div v-if="selected" class="card-ab p-4 flex flex-col gap-2">
        <div class="flex items-center gap-2">
          <span class="signal-dot" :style="{ background: roleColor(selected) }"></span>
          <span class="font-mono text-[13px] font-medium text-ink">{{ selected.type }}</span>
          <span class="font-mono text-[11px] text-ink-tertiary">node {{ selected.id }}</span>
          <span v-if="selected.title" class="text-xs font-light text-ink-secondary truncate">“{{ selected.title }}”</span>
          <span class="flex-1"></span>
          <button class="font-mono text-[13px] text-ink-tertiary hover:text-signal" @click="selectedId = null">✕</button>
        </div>
        <div v-if="selected.fields.length" class="grid gap-x-4 gap-y-1 font-mono text-xs" style="grid-template-columns: auto 1fr">
          <template v-for="f in selected.fields" :key="f.field">
            <span class="text-ink-tertiary">{{ f.field }}</span>
            <span v-if="f.linked" class="text-ink-secondary">
              ← {{ sourceLabel(selected.links.find((l) => l.field === f.field)) }}
            </span>
            <span v-else class="text-ink-secondary break-all">{{ f.full || '—' }}</span>
          </template>
        </div>
        <div v-else class="font-mono text-[11px] text-ink-tertiary">no inputs</div>
      </div>

      <pre
        v-if="showJson"
        class="bg-base border border-line rounded-sm p-3 font-mono text-[11px] text-ink-secondary overflow-auto max-h-80"
      >{{ JSON.stringify(graph.graph, null, 2) }}</pre>
    </template>
  </div>
</template>

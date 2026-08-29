<script setup>
/**
 * Review Desk — one screen, three lanes.
 *
 * Shots, duplicates and face clusters used to be separate destinations. They are
 * the same job (decide, route, move on), so the design puts them on one desk and
 * lets the lane tabs carry the counts.
 */
import { ref, computed, onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import ReviewQueue from '@/components/ReviewQueue.vue'
import VariationsQueue from '@/components/VariationsQueue.vue'
import PersonNamer from '@/components/PersonNamer.vue'

const route = useRoute()
const router = useRouter()

const LANES = ['shots', 'duplicates', 'faces']
const lane = ref(LANES.includes(route.query.lane) ? route.query.lane : 'shots')

watch(() => route.query.lane, (v) => {
  if (LANES.includes(v)) lane.value = v
})

function goLane(l) {
  lane.value = l
  router.replace({ name: 'review', query: { ...route.query, lane: l === 'shots' ? undefined : l } })
}

const stats = ref({ pending_review: 0, unnamed_people: 0, confirmed: 0, total_shots: 0 })
const dupeCount = ref(0)

async function loadData() {
  try {
    const res = await fetch('/api/organize/stats')
    if (res.ok) stats.value = await res.json()
  } catch { /* the lanes each report their own emptiness */ }
  try {
    const res = await fetch('/api/shots/similar-groups?offset=0&limit=1')
    if (res.ok) dupeCount.value = (await res.json()).total || 0
  } catch { dupeCount.value = 0 }
}

const lanes = computed(() => [
  { id: 'shots', label: 'Shots', count: stats.value.pending_review || 0 },
  { id: 'duplicates', label: 'Duplicates', count: dupeCount.value || 0 },
  { id: 'faces', label: 'Faces', count: stats.value.unnamed_people || 0 },
])

const progress = computed(() => {
  const total = stats.value.total_shots || 0
  if (!total) return 'nothing scanned yet'
  const pct = Math.round(((stats.value.confirmed || 0) / total) * 100)
  return `${pct}% of library filed`
})

onMounted(loadData)
defineExpose({ loadData })
</script>

<template>
  <div class="flex flex-col flex-1 min-h-0">
    <!-- Lane tabs + progress -->
    <div class="border-b border-line px-4 md:px-8 flex items-center gap-6 flex-none">
      <div class="flex gap-1">
        <button
          v-for="ln in lanes"
          :key="ln.id"
          class="flex items-center gap-2 px-3 py-4 border-b-2 text-[13px] transition-colors"
          :class="lane === ln.id
            ? 'border-signal text-ink font-medium'
            : 'border-transparent text-ink-secondary hover:text-ink'"
          @click="goLane(ln.id)"
        >
          {{ ln.label }}
          <span class="font-mono text-[11px] text-ink-tertiary">{{ ln.count }}</span>
        </button>
      </div>
      <div class="flex-1"></div>
      <div class="font-mono text-xs text-ink-tertiary hidden sm:block">{{ progress }}</div>
    </div>

    <ReviewQueue v-if="lane === 'shots'" class="flex-1 min-h-0" @changed="loadData" />
    <VariationsQueue v-else-if="lane === 'duplicates'" class="flex-1 min-h-0" @changed="loadData" />
    <PersonNamer v-else inline class="flex-1 min-h-0" @changed="loadData" />
  </div>
</template>

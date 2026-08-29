<script setup>
import { computed } from 'vue'

const props = defineProps({
  shot: {
    type: Object,
    required: true,
    // Expected shape: { id, thumbnail_url, file_count, primary_person_name, review_status }
  },
})

const statusColor = computed(() => {
  switch (props.shot.review_status) {
    case 'confirmed': return 'var(--status-ready)'
    case 'unsorted': return 'var(--status-pending)'
    default: return 'var(--status-degraded)'
  }
})
</script>

<template>
  <div class="relative aspect-square bg-surface border border-line rounded overflow-hidden cursor-pointer transition-colors hover:border-line-strong">
    <img
      v-if="shot.thumbnail_url"
      :src="shot.thumbnail_url"
      :title="shot.description || ''"
      class="w-full h-full object-cover"
      loading="lazy"
    />
    <div v-else class="w-full h-full flex items-center justify-center font-mono text-[11px] text-ink-tertiary">
      no thumbnail
    </div>

    <!-- Status is a signal light, not a caption. -->
    <span class="absolute top-2 left-2 signal-dot" :style="{ background: statusColor }"></span>

    <span
      v-if="shot.file_count > 1"
      class="absolute top-1.5 right-2 font-mono text-[10px] text-ink-secondary bg-base border border-line rounded-sm px-1"
    >×{{ shot.file_count }}</span>

    <span
      class="absolute inset-x-0 bottom-0 px-2 py-1 font-mono text-[10px] truncate bg-base border-t border-line"
      :class="shot.primary_person_name ? 'text-ink-secondary' : 'text-ink-tertiary'"
    >{{ shot.primary_person_name || 'unsorted' }}</span>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { cn } from '@/lib/utils'

const props = defineProps({
  shot: {
    type: Object,
    required: true,
    // Expected shape: { id, thumbnail_url, file_count, primary_person_name, review_status }
  },
})

const statusDot = computed(() => {
  switch (props.shot.review_status) {
    case 'confirmed':
      return 'bg-emerald-500'
    case 'pending':
    default:
      return 'bg-yellow-500'
  }
})

const statusLabel = computed(() => {
  switch (props.shot.review_status) {
    case 'confirmed':
      return 'Confirmed'
    case 'pending':
    default:
      return 'Pending'
  }
})
</script>

<template>
  <div class="group relative aspect-square bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800 hover:border-indigo-500 transition-colors cursor-pointer">
    <!-- Thumbnail -->
    <img
      v-if="shot.thumbnail_url"
      :src="shot.thumbnail_url"
      class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300"
      loading="lazy"
    />
    <div
      v-else
      class="w-full h-full flex items-center justify-center bg-zinc-800"
    >
      <span class="text-zinc-600 text-sm">No thumbnail</span>
    </div>

    <!-- Overlay gradient at bottom -->
    <div class="absolute inset-x-0 bottom-0 h-16 bg-gradient-to-t from-black/70 to-transparent pointer-events-none" />

    <!-- File count badge (top-right) -->
    <div
      v-if="shot.file_count >= 1"
      class="absolute top-2 right-2 px-1.5 py-0.5 rounded bg-black/60 backdrop-blur-sm border border-white/10 text-xs font-medium text-white"
    >
      {{ shot.file_count }} {{ shot.file_count === 1 ? 'file' : 'files' }}
    </div>

    <!-- Review status dot (top-left) -->
    <div class="absolute top-2 left-2 flex items-center gap-1.5 px-1.5 py-0.5 rounded bg-black/60 backdrop-blur-sm border border-white/10">
      <div :class="cn('w-2 h-2 rounded-full', statusDot)" />
      <span class="text-[10px] font-medium text-zinc-300">{{ statusLabel }}</span>
    </div>

    <!-- Person label (bottom-left) -->
    <div
      v-if="shot.primary_person_name"
      class="absolute bottom-2 left-2 right-2 truncate text-xs font-medium text-white"
    >
      {{ shot.primary_person_name }}
    </div>
    <div
      v-else
      class="absolute bottom-2 left-2 right-2 truncate text-xs font-medium text-zinc-400 italic"
    >
      Unsorted
    </div>
  </div>
</template>

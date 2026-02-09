<script setup>
import { DialogDescription } from 'radix-vue'
import { cn } from '@/lib/utils'

const props = defineProps({
  class: { type: null, default: '' },
})
</script>

<template>
  <DialogDescription
    v-bind="props"
    :class="cn('text-sm text-zinc-400', props.class)"
  >
    <slot />
  </DialogDescription>
</template>

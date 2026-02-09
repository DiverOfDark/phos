<script setup>
import { DialogTitle } from 'radix-vue'
import { cn } from '@/lib/utils'

const props = defineProps({
  class: { type: null, default: '' },
})
</script>

<template>
  <DialogTitle
    v-bind="props"
    :class="cn('text-lg font-semibold text-white', props.class)"
  >
    <slot />
  </DialogTitle>
</template>

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
    :class="cn(
      'text-lg font-semibold leading-none tracking-tight text-white',
      props.class,
    )"
  >
    <slot />
  </DialogTitle>
</template>

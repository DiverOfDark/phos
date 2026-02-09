<script setup>
import { TabsList } from 'radix-vue'
import { cn } from '@/lib/utils'

const props = defineProps({
  class: { type: null, default: '' },
})
</script>

<template>
  <TabsList
    v-bind="props"
    :class="cn(
      'inline-flex h-10 items-center justify-center rounded-xl bg-zinc-900 p-1 text-zinc-400',
      props.class,
    )"
  >
    <slot />
  </TabsList>
</template>

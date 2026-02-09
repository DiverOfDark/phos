<script setup>
import { 
  TabsContent, 
  TabsList, 
  TabsRoot, 
  TabsTrigger, 
  useForwardPropsEmits 
} from 'radix-vue'
import { cn } from '@/lib/utils'

const props = defineProps({
  class: { type: null, default: '' },
})

const emits = defineEmits(['update:modelValue'])
const forwarded = useForwardPropsEmits(props, emits)
</script>

<template>
  <TabsRoot
    v-bind="forwarded"
    :class="cn('w-full', props.class)"
  >
    <slot />
  </TabsRoot>
</template>

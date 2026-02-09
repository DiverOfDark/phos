<script setup>
import {
  DialogContent,
  DialogPortal,
  useForwardPropsEmits,
} from 'radix-vue'
import { X } from 'lucide-vue-next'
import DialogOverlay from '../dialog/DialogOverlay.vue'
import { cn } from '@/lib/utils'

const props = defineProps({
  class: { type: null, default: '' },
})

const emits = defineEmits(['escapeKeyDown', 'pointerDownOutside', 'focusBlur', 'interactOutside', 'openAutoFocus', 'closeAutoFocus'])
const forwarded = useForwardPropsEmits(props, emits)
</script>

<template>
  <DialogPortal>
    <DialogOverlay />
    <DialogContent
      v-bind="forwarded"
      :class="cn(
        'fixed inset-y-0 right-0 z-50 h-full w-3/4 border-l border-white/10 bg-zinc-950 p-6 shadow-lg transition duration-300 ease-in-out data-[state=closed]:animate-out data-[state=open]:animate-in data-[state=closed]:slide-out-to-right data-[state=open]:slide-in-from-right sm:max-w-md',
        props.class
      )"
    >
      <slot />

      <DialogClose
        class="absolute right-4 top-4 rounded-sm opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 disabled:pointer-events-none data-[state=open]:bg-secondary"
      >
        <X class="h-4 w-4 text-zinc-400" />
        <span class="sr-only">Close</span>
      </DialogClose>
    </DialogContent>
  </DialogPortal>
</template>

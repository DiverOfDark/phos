<script setup>
import { ref } from 'vue'
import { useRoute } from 'vue-router'
import { useAuth } from '@/composables/useAuth'
import { Button } from '@/components/ui/button'
import { LogIn, AlertCircle } from 'lucide-vue-next'

const route = useRoute()
const { login } = useAuth()

const error = ref(route.query.error || '')
</script>

<template>
  <div class="min-h-screen bg-zinc-950 flex items-center justify-center p-4">
    <div class="w-full max-w-sm space-y-8 text-center">
      <!-- Logo -->
      <div class="flex flex-col items-center gap-3">
        <div class="w-16 h-16 bg-indigo-600 rounded-2xl flex items-center justify-center shadow-lg shadow-indigo-500/20">
          <span class="text-white font-black text-3xl">P</span>
        </div>
        <h1 class="text-2xl font-bold text-white tracking-tight">Phos</h1>
        <p class="text-sm text-zinc-500">Sign in to access your photo library</p>
      </div>

      <!-- Error -->
      <div
        v-if="error"
        class="flex items-start gap-2 p-3 rounded-xl bg-red-500/10 border border-red-500/20 text-left"
      >
        <AlertCircle class="w-4 h-4 text-red-500 mt-0.5 shrink-0" />
        <p class="text-sm text-red-400">{{ error }}</p>
      </div>

      <!-- Sign in button -->
      <Button
        class="w-full bg-indigo-600 hover:bg-indigo-500 text-white shadow-lg shadow-indigo-500/20 rounded-xl gap-2 h-12 text-base"
        @click="login"
      >
        <LogIn class="w-5 h-5" />
        Sign in with SSO
      </Button>

      <p class="text-[10px] text-zinc-600 font-bold tracking-[0.2em] uppercase">
        Phos v0.1.0-alpha
      </p>
    </div>
  </div>
</template>

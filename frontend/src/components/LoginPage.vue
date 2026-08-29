<script setup>
import { ref } from 'vue'
import { useRoute } from 'vue-router'
import { useAuth } from '@/composables/useAuth'

const version = __PHOS_VERSION__
const route = useRoute()
const { login } = useAuth()

const error = ref(route.query.error || '')
</script>

<template>
  <div class="min-h-screen flex items-center justify-center p-4 bg-base">
    <div class="w-[360px] max-w-full flex flex-col gap-6">
      <div class="flex items-center gap-4">
        <img src="/phos.svg" alt="Phos" class="w-10 h-10 rounded" />
        <div>
          <div class="font-heading text-[22px] font-bold tracking-[-0.01em] text-ink">Phos</div>
          <div class="font-mono text-xs text-ink-tertiary">media console</div>
        </div>
      </div>

      <div class="card-ab p-6 flex flex-col gap-4">
        <div class="text-[13px] font-light text-ink-secondary">
          Sign in to your library. Authentication is handled by your identity provider.
        </div>

        <div v-if="error" class="flex items-center gap-2 font-mono text-xs text-error">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--status-error)"></span>
          {{ error }}
        </div>

        <button
          class="bg-signal text-signal-fg rounded px-4 py-2.5 text-sm font-medium hover:bg-signal-hover transition-colors"
          @click="login"
        >Continue with SSO</button>

        <div class="flex items-center gap-2 font-mono text-xs text-ink-tertiary">
          <span class="signal-dot" style="width:6px;height:6px;background:var(--status-ready)"></span>
          oidc
        </div>
      </div>

      <div class="font-mono text-xs text-ink-tertiary">phos {{ version }} · self-hosted</div>
    </div>
  </div>
</template>

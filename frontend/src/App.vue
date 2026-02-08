<script setup>
</script>

<template>
  <div class="min-h-screen bg-zinc-950 text-zinc-100 flex flex-col items-center justify-center p-4">
    <h1 class="text-4xl font-black tracking-tighter mb-2">PHOS</h1>
    <p class="text-zinc-400 font-medium">AI Photo & Video Manager</p>
    <div class="mt-8 bg-zinc-900 border border-zinc-800 rounded-2xl p-6 w-full max-w-md shadow-2xl">
      <div class="flex items-center space-x-4 mb-4">
        <div class="w-12 h-12 bg-indigo-600 rounded-full flex items-center justify-center font-bold text-xl">P</div>
        <div>
          <h2 class="font-bold">System Status</h2>
          <p class="text-xs text-zinc-500">Backend: <span class="text-emerald-500">Online</span></p>
        </div>
      </div>
      <p class="text-sm text-zinc-400 leading-relaxed">
        Welcome to the Phos early preview. We are currently indexing your local memories.
      </p>
      <button class="mt-6 w-full bg-indigo-600 hover:bg-indigo-500 text-white font-bold py-3 rounded-xl transition-all active:scale-95">
        Explore Gallery
      </button>
    </div>
  </div>
</template>

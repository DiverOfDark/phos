<script setup>
import { ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { ScrollArea } from '@/components/ui/scroll-area'
import { 
  Activity, 
  Image as ImageIcon, 
  Users, 
  Settings, 
  Search, 
  Upload, 
  LayoutGrid,
  RefreshCw
} from 'lucide-vue-next'

const stats = ref([
  { name: 'Total Media', value: '0', icon: ImageIcon, description: 'Images and videos indexed' },
  { name: 'Detected People', value: '0', icon: Users, description: 'Face clusters identified' },
  { name: 'System Status', value: 'Idle', icon: Activity, description: 'Backend processing state' },
])

const isScanning = ref(false)

const startScan = async () => {
  isScanning.value = true
  // Mocking scan start
  setTimeout(() => {
    isScanning.value = false
  }, 3000)
}
</script>

<template>
  <div class="min-h-screen bg-zinc-950 text-zinc-50 font-sans selection:bg-indigo-500/30">
    <!-- Glass Sidebar/Nav -->
    <header class="border-b border-white/5 bg-zinc-950/50 backdrop-blur-xl sticky top-0 z-50">
      <div class="max-w-7xl mx-auto px-4 h-16 flex items-center justify-between">
        <div class="flex items-center gap-8">
          <div class="flex items-center gap-2.5">
            <div class="w-9 h-9 bg-indigo-600 rounded-xl flex items-center justify-center shadow-lg shadow-indigo-500/20">
              <span class="text-white font-black text-lg">P</span>
            </div>
            <span class="text-xl font-bold tracking-tight text-white">Phos</span>
          </div>
          
          <nav class="hidden md:flex items-center gap-1">
            <Button variant="ghost" class="text-zinc-400 hover:text-white hover:bg-white/5 gap-2 px-3">
              <LayoutGrid class="w-4 h-4" />
              Library
            </Button>
            <Button variant="ghost" class="text-zinc-400 hover:text-white hover:bg-white/5 gap-2 px-3">
              <Users class="w-4 h-4" />
              People
            </Button>
            <Button variant="ghost" class="text-zinc-400 hover:text-white hover:bg-white/5 gap-2 px-3">
              <Activity class="w-4 h-4" />
              Timeline
            </Button>
          </nav>
        </div>

        <div class="flex items-center gap-2">
          <div class="relative hidden sm:block mr-2">
            <Search class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-zinc-500" />
            <input 
              type="text" 
              placeholder="Search memories..." 
              class="bg-zinc-900 border-none rounded-full pl-9 pr-4 py-1.5 text-sm w-64 focus:ring-2 focus:ring-indigo-500/50 outline-none transition-all placeholder:text-zinc-600"
            />
          </div>
          <Button variant="ghost" size="icon" class="text-zinc-400 hover:text-white rounded-full">
            <Settings class="w-5 h-5" />
          </Button>
          <div class="h-8 w-[1px] bg-white/10 mx-1"></div>
          <Button class="bg-indigo-600 hover:bg-indigo-500 text-white shadow-lg shadow-indigo-500/20 rounded-xl gap-2 px-4">
            <Upload class="w-4 h-4" />
            Import
          </Button>
        </div>
      </div>
    </header>

    <main class="max-w-7xl mx-auto p-6 md:p-8">
      <!-- Welcome Header -->
      <div class="mb-10">
        <h2 class="text-3xl font-bold tracking-tight text-white mb-2 text-glow">Welcome back, Kirill</h2>
        <p class="text-zinc-400">Your personal AI-curated photo laboratory is ready.</p>
      </div>

      <!-- Stats Grid -->
      <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 mb-12">
        <Card v-for="stat in stats" :key="stat.name" class="bg-zinc-900/40 border-white/5 backdrop-blur-sm group hover:border-indigo-500/30 transition-all duration-300">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400 group-hover:text-zinc-300">{{ stat.name }}</CardTitle>
            <component :is="stat.icon" class="w-4 h-4 text-zinc-500 group-hover:text-indigo-400 transition-colors" />
          </CardHeader>
          <CardContent>
            <div class="text-3xl font-bold text-white tracking-tight">{{ stat.value }}</div>
            <p class="text-xs text-zinc-500 mt-1.5">{{ stat.description }}</p>
          </CardContent>
        </Card>
      </div>

      <!-- Main Action Area -->
      <div class="relative">
        <div class="flex items-center justify-between mb-6">
          <h3 class="text-lg font-semibold text-white/90">Recent Discovery</h3>
          <Button variant="link" class="text-indigo-400 hover:text-indigo-300 p-0 h-auto font-medium">
            Browse all
          </Button>
        </div>

        <ScrollArea class="h-[500px] w-full rounded-3xl border border-white/5 bg-zinc-900/20 backdrop-blur-sm relative overflow-hidden group">
          <!-- Background Decoration -->
          <div class="absolute inset-0 bg-gradient-to-br from-indigo-500/5 via-transparent to-purple-500/5 opacity-0 group-hover:opacity-100 transition-opacity duration-700 pointer-events-none"></div>
          
          <div class="flex flex-col items-center justify-center h-full text-center p-12 space-y-6 relative z-10">
            <div class="relative">
              <div class="absolute inset-0 bg-indigo-500 blur-3xl opacity-10 animate-pulse"></div>
              <div class="w-20 h-20 bg-zinc-900 rounded-2xl flex items-center justify-center border border-white/5 shadow-2xl relative">
                <ImageIcon class="w-10 h-10 text-zinc-700 group-hover:text-indigo-500 transition-colors duration-500" />
              </div>
            </div>
            
            <div class="max-w-xs">
              <p class="text-xl font-bold text-white mb-2">No memories found</p>
              <p class="text-zinc-500 text-sm leading-relaxed">
                Connect your library folder to start the AI-powered indexing and face clustering process.
              </p>
            </div>

            <Button 
              @click="startScan"
              :disabled="isScanning"
              class="bg-white text-black hover:bg-zinc-200 font-bold px-8 py-6 rounded-2xl transition-all active:scale-95 disabled:opacity-50 h-auto"
            >
              <RefreshCw v-if="isScanning" class="w-5 h-5 mr-2 animate-spin" />
              {{ isScanning ? 'Initializing Intelligence...' : 'Scan test_library' }}
            </Button>
          </div>
        </ScrollArea>
      </div>
    </main>

    <!-- Footer Meta -->
    <footer class="mt-auto py-8 border-t border-white/5 text-center">
      <p class="text-xs text-zinc-600 font-medium tracking-widest uppercase">
        Encrypted Local Storage &bull; Phos v0.1.0-alpha
      </p>
    </footer>
  </div>
</template>

<style>
.text-glow {
  text-shadow: 0 0 30px rgba(99, 102, 241, 0.2);
}

/* Custom Scrollbar */
::-webkit-scrollbar {
  width: 8px;
}
::-webkit-scrollbar-track {
  background: transparent;
}
::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.1);
  border-radius: 10px;
}
::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.2);
}
</style>

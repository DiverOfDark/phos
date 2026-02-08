<script setup>
import { ref, onMounted } from 'vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Activity, Image as ImageIcon, Users, Settings } from 'lucide-vue-next'
import Gallery from '@/components/Gallery.vue'
import PeopleList from '@/components/PeopleList.vue'

const currentView = ref('library')
const stats = ref({
  total: 0,
  people: 0,
  status: 'Online'
})

async function triggerScan() {
  await fetch('http://localhost:3000/api/scan', { method: 'POST' })
}

async function fetchStats() {
    // Placeholder for actual stats API
}

onMounted(fetchStats)
</script>

<template>
  <div class="min-h-screen bg-black text-zinc-100 flex flex-col font-sans">
    <!-- Navbar -->
    <header class="border-b border-zinc-800 p-4 flex items-center justify-between sticky top-0 bg-black/80 backdrop-blur-md z-10">
      <div class="flex items-center space-x-2">
        <div class="w-8 h-8 bg-indigo-600 rounded-lg flex items-center justify-center font-black">P</div>
        <h1 class="text-xl font-bold tracking-tighter">PHOS</h1>
      </div>
      <nav class="hidden md:flex items-center space-x-6 text-sm font-medium text-zinc-400">
        <button 
          @click="currentView = 'library'" 
          :class="['hover:text-white transition-colors flex items-center space-x-2', currentView === 'library' ? 'text-zinc-100' : '']"
        >
          <ImageIcon class="w-4 h-4" />
          <span>Library</span>
        </button>
        <button 
          @click="currentView = 'people'" 
          :class="['hover:text-white transition-colors flex items-center space-x-2', currentView === 'people' ? 'text-zinc-100' : '']"
        >
          <Users class="w-4 h-4" />
          <span>People</span>
        </button>
      </nav>
      <Button variant="ghost" size="icon">
        <Settings class="w-5 h-5" />
      </Button>
    </header>

    <!-- Main Content -->
    <main class="flex-1 p-6 max-w-7xl mx-auto w-full">
      <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-8">
        <Card class="bg-zinc-900 border-zinc-800 text-white">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400">Total Media</CardTitle>
            <ImageIcon class="w-4 h-4 text-zinc-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold">{{ stats.total }}</div>
          </CardContent>
        </Card>
        <Card class="bg-zinc-900 border-zinc-800 text-white">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400">People</CardTitle>
            <Users class="w-4 h-4 text-zinc-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold">{{ stats.people }}</div>
          </CardContent>
        </Card>
        <Card class="bg-zinc-900 border-zinc-800 text-white">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400">Status</CardTitle>
            <Activity class="w-4 h-4 text-zinc-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold">{{ stats.status }}</div>
          </CardContent>
        </Card>
      </div>

      <div v-if="currentView === 'library'">
        <div class="flex items-center justify-between mb-4">
          <h3 class="text-lg font-semibold">Library</h3>
          <Button @click="triggerScan" size="sm" variant="outline" class="border-zinc-800 bg-zinc-900 hover:bg-zinc-800">
            Scan Now
          </Button>
        </div>
        <Gallery />
      </div>

      <div v-else-if="currentView === 'people'">
        <h3 class="text-lg font-semibold mb-4">People</h3>
        <PeopleList />
      </div>
    </main>
  </div>
</template>

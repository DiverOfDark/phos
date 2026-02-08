<script setup>
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Activity, Image as ImageIcon, Users, Settings } from 'lucide-vue-next'
</script>

<template>
  <div class="min-h-screen bg-black text-zinc-100 flex flex-col">
    <!-- Navbar -->
    <header class="border-b border-zinc-800 p-4 flex items-center justify-between sticky top-0 bg-black/80 backdrop-blur-md z-10">
      <div class="flex items-center space-x-2">
        <div class="w-8 h-8 bg-indigo-600 rounded-lg flex items-center justify-center font-black">P</div>
        <h1 class="text-xl font-bold tracking-tighter">PHOS</h1>
      </div>
      <nav class="hidden md:flex items-center space-x-6 text-sm font-medium text-zinc-400">
        <a href="#" class="text-zinc-100 hover:text-white transition-colors flex items-center space-x-2">
          <ImageIcon class="w-4 h-4" />
          <span>Library</span>
        </a>
        <a href="#" class="hover:text-white transition-colors flex items-center space-x-2">
          <Users class="w-4 h-4" />
          <span>People</span>
        </a>
        <a href="#" class="hover:text-white transition-colors flex items-center space-x-2">
          <Activity class="w-4 h-4" />
          <span>Activity</span>
        </a>
      </nav>
      <Button variant="ghost" size="icon">
        <Settings class="w-5 h-5" />
      </Button>
    </header>

    <!-- Main Content -->
    <main class="flex-1 p-6 max-w-7xl mx-auto w-full">
      <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-8">
        <Card class="bg-zinc-900 border-zinc-800">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400">Total Media</CardTitle>
            <ImageIcon class="w-4 h-4 text-zinc-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold">0</div>
            <p class="text-xs text-zinc-500 mt-1">Ready to scan folders</p>
          </CardContent>
        </Card>
        <Card class="bg-zinc-900 border-zinc-800">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400">Detected People</CardTitle>
            <Users class="w-4 h-4 text-zinc-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold">0</div>
            <p class="text-xs text-zinc-500 mt-1">No faces indexed yet</p>
          </CardContent>
        </Card>
        <Card class="bg-zinc-900 border-zinc-800">
          <CardHeader class="flex flex-row items-center justify-between pb-2 space-y-0">
            <CardTitle class="text-sm font-medium text-zinc-400">System Load</CardTitle>
            <Activity class="w-4 h-4 text-zinc-500" />
          </CardHeader>
          <CardContent>
            <div class="text-2xl font-bold">Idle</div>
            <p class="text-xs text-emerald-500 mt-1">Backend Online</p>
          </CardContent>
        </Card>
      </div>

      <div class="flex items-center justify-between mb-4">
        <h3 class="text-lg font-semibold">Recent Media</h3>
        <Button size="sm" variant="outline" class="border-zinc-800 bg-zinc-900 hover:bg-zinc-800">
          View All
        </Button>
      </div>

      <ScrollArea class="h-[400px] w-full rounded-md border border-zinc-800 bg-zinc-950 p-8 flex flex-col items-center justify-center text-center">
        <div class="flex flex-col items-center justify-center space-y-4">
          <div class="w-16 h-16 bg-zinc-900 rounded-full flex items-center justify-center">
            <ImageIcon class="w-8 h-8 text-zinc-700" />
          </div>
          <div>
            <p class="text-zinc-400 font-medium">Your library is empty</p>
            <p class="text-sm text-zinc-600">Drag and drop photos here or scan a directory</p>
          </div>
          <Button class="bg-indigo-600 hover:bg-indigo-500 text-white">
            Scan Directory
          </Button>
        </div>
      </ScrollArea>
    </main>
  </div>
</template>

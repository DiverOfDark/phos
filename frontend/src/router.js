import { createRouter, createWebHistory } from 'vue-router'
import Gallery from './components/Gallery.vue'
import PeopleList from './components/PeopleList.vue'

const routes = [
  { path: '/', component: Gallery },
  { path: '/people', component: PeopleList },
  { path: '/timeline', component: { template: '<div class="p-8 text-center text-zinc-500 uppercase tracking-widest font-bold">Timeline coming soon...</div>' } },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

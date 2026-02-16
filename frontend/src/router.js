import { createRouter, createWebHistory } from 'vue-router'
import OrganizeDashboard from './components/OrganizeDashboard.vue'
import ReviewQueue from './components/ReviewQueue.vue'
import ShotDetail from './components/ShotDetail.vue'
import PersonDetail from './components/PersonDetail.vue'
import Gallery from './components/Gallery.vue'
import PeopleList from './components/PeopleList.vue'
import Timeline from './components/Timeline.vue'

const routes = [
  {
    path: '/',
    name: 'organize',
    component: OrganizeDashboard,
    meta: { view: 'organize' },
  },
  {
    path: '/review',
    name: 'review',
    component: ReviewQueue,
    meta: { view: 'review' },
  },
  {
    path: '/shot/:id',
    name: 'shot-detail',
    component: ShotDetail,
    meta: { view: 'organize' },
  },
  {
    path: '/person/:id',
    name: 'person-detail',
    component: PersonDetail,
    meta: { view: 'people' },
  },
  {
    path: '/browse',
    name: 'browse',
    component: Gallery,
    meta: { view: 'browse' },
  },
  {
    path: '/people',
    name: 'people',
    component: PeopleList,
    meta: { view: 'people' },
  },
  {
    path: '/timeline',
    name: 'timeline',
    component: Timeline,
    meta: { view: 'timeline' },
  },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

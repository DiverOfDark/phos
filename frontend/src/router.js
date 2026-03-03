import { createRouter, createWebHistory } from 'vue-router'
import { useAuth } from '@/composables/useAuth'
import OrganizeDashboard from './components/OrganizeDashboard.vue'
import ReviewQueue from './components/ReviewQueue.vue'
import VariationsQueue from './components/VariationsQueue.vue'
import ShotDetail from './components/ShotDetail.vue'
import PersonDetail from './components/PersonDetail.vue'
import PeopleList from './components/PeopleList.vue'
import LoginPage from './components/LoginPage.vue'
import WorkflowsPage from './components/WorkflowsPage.vue'

const routes = [
  {
    path: '/login',
    name: 'login',
    component: LoginPage,
    meta: { public: true },
  },
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
    path: '/variations',
    name: 'variations',
    component: VariationsQueue,
    meta: { view: 'variations' },
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
    path: '/people',
    name: 'people',
    component: PeopleList,
    meta: { view: 'people' },
  },
  {
    path: '/workflows',
    name: 'workflows',
    component: WorkflowsPage,
    meta: { view: 'workflows' },
  },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

router.beforeEach(async (to) => {
  if (to.meta.public) return true

  const { isAuthenticated, checked, fetchUser } = useAuth()
  if (!checked.value) {
    await fetchUser()
  }
  if (!isAuthenticated.value) {
    return '/login'
  }
})

import { createRouter, createWebHistory } from 'vue-router'
import { useAuth } from '@/composables/useAuth'
import OrganizeDashboard from './components/OrganizeDashboard.vue'
import ShotDetail from './components/ShotDetail.vue'
import PersonDetail from './components/PersonDetail.vue'
import PeopleList from './components/PeopleList.vue'
import LoginPage from './components/LoginPage.vue'
import WorkflowsPage from './components/WorkflowsPage.vue'
import ReviewDesk from './components/ReviewDesk.vue'
import SettingsPage from './components/SettingsPage.vue'

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
    meta: { view: 'overview' },
  },
  {
    path: '/review',
    name: 'review',
    component: ReviewDesk,
    meta: { view: 'review' },
  },
  {
    // The duplicates queue is a lane of the Review Desk now; the old route
    // stays as a redirect so bookmarks keep working.
    path: '/variations',
    redirect: { name: 'review', query: { lane: 'duplicates' } },
  },
  {
    path: '/shot/:id',
    name: 'shot-detail',
    component: ShotDetail,
    meta: { view: 'overview' },
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
  {
    path: '/settings',
    name: 'settings',
    component: SettingsPage,
    meta: { view: 'settings' },
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

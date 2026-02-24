import { ref, computed } from 'vue'

const user = ref(null)
const checked = ref(false)
const authEnabled = ref(true)

export function useAuth() {
  const isAuthenticated = computed(() => !authEnabled.value || !!user.value)

  async function fetchUser() {
    try {
      const res = await fetch('/api/auth/me')
      if (res.ok) {
        user.value = await res.json()
        authEnabled.value = true
      } else if (res.status === 401) {
        user.value = null
        authEnabled.value = true
      } else {
        // 404 or other — auth not enabled on backend
        authEnabled.value = false
        user.value = null
      }
    } catch {
      // Network error / backend down — assume no auth
      authEnabled.value = false
      user.value = null
    }
    checked.value = true
  }

  function login() {
    window.location.href = '/api/auth/login'
  }

  function logout() {
    window.location.href = '/api/auth/logout'
  }

  return { user, isAuthenticated, authEnabled, checked, fetchUser, login, logout }
}

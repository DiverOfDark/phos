// AppBahn type stack, bundled rather than pulled from a CDN — a self-hosted
// library on a LAN must not depend on fonts.googleapis.com to look right.
import '@fontsource-variable/geist'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import { createApp } from 'vue'
import './style.css'
import App from './App.vue'
import { router } from './router'

createApp(App).use(router).mount('#app')

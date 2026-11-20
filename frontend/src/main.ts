import { createApp } from 'vue'
import App from './app/App.vue'
import { router } from './app/router'
import { initializeTheme } from './app/theme'
import './app/style.css'
initializeTheme()
createApp(App).use(router).mount('#app')

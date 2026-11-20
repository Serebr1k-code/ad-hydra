<script setup lang="ts">
import { ref,onMounted,onUnmounted } from 'vue'
import { Button } from '@/components/ui/button'
import LoginForm from './LoginForm.vue'
import ConsoleLayout from '@/components/layout/ConsoleLayout.vue'
import { useSession } from './useSession'
import { closeEditor } from '@/shared/editor'
import { useTheme } from '@/app/theme'
const {authenticated,busy,error,initialize,signIn,signOut} = useSession()
const {theme,toggleTheme}=useTheme()
const status = ref('Signed out'),proxyConnected=ref<boolean|null>(null)
function proxyStatus(value:string,connected:boolean){status.value=value;proxyConnected.value=connected}
function signedOut() { authenticated.value = false; status.value = 'Signed out'; proxyConnected.value=null; closeEditor() }
onMounted(() => { window.addEventListener('hydra-session-changed',signedOut); void initialize() })
onUnmounted(() => window.removeEventListener('hydra-session-changed',signedOut))
</script>
<template>
 <header id="hydra-header" :data-proxy-connected="authenticated?proxyConnected:null" class="flex items-center gap-3 border-b px-5 py-4 sm:gap-4 sm:px-8" :class="authenticated&&proxyConnected===false?'border-red-500 bg-red-950 text-red-100':''"><strong class="tracking-widest">HYDRA</strong><span id="status" class="flex-1 text-xs sm:text-sm" :class="authenticated&&proxyConnected===false?'text-red-100':'text-muted-foreground'">{{authenticated?status:'Signed out'}}</span><Button id="theme-toggle" variant="outline" class="h-8 px-2.5 text-xs" :aria-label="`Switch to ${theme==='dark'?'OLED':'Dark'} theme`" @click="toggleTheme">{{theme==='dark'?'OLED':'Dark'}}</Button><Button id="logout" v-show="authenticated" :hidden="!authenticated" variant="outline" :disabled="busy" @click="signOut">Sign out</Button></header>
 <main class="mx-auto max-w-[1440px] p-4 sm:p-6">
  <LoginForm v-if="!authenticated" :busy="busy" @submit="signIn"/>
  <ConsoleLayout v-else @status="proxyStatus"/>
  <p id="error" role="alert" class="mt-4 text-sm text-destructive">{{error}}</p>
 </main>
</template>

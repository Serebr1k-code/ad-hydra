<script setup lang="ts">
import { ref,onMounted,onUnmounted } from 'vue'
import { useRoute,useRouter } from 'vue-router'
import { Button } from '@/components/ui/button'
import {loadUserDirectory,clearUserDirectory} from '@/shared/users/directory'
import { api,events,errorMessage } from '@/shared/api/client'
const emit = defineEmits<{status:[value:string,connected:boolean]}>()
const route = useRoute(), router = useRouter(), notice = ref('')
const sections = [{name:'Services',path:'/services'},{name:'Filters',path:'/filters'},{name:'Traffic',path:'/traffic'},{name:'Protocols',path:'/protocols'},{name:'Farm',path:'/farm'},{name:'Flags',path:'/flags'},{name:'Users / audit',path:'/users'},{name:'Settings',path:'/settings'}]
const abort = new AbortController()
let mounted=true
let interval: ReturnType<typeof setInterval> | undefined
async function status() {
 try {
  const s = await api<{connected:boolean;active_flows:number;telemetry_dropped:number;indexing_dropped:number;applied_services_revision:number;applied_rules_revision:number}>('/system/proxy')
  if(!mounted)return
  emit('status',`Proxy ${s.connected?'connected':'disconnected'} · ${s.active_flows} flows · ${s.telemetry_dropped+s.indexing_dropped} telemetry lost · revision ${s.applied_services_revision}/${s.applied_rules_revision}`,s.connected)
 } catch (e) { if(mounted){notice.value = errorMessage(e);emit('status','Proxy connection unavailable',false)} }
}
onMounted(() => {
 void loadUserDirectory().catch(e=>{notice.value=errorMessage(e)})
 void status(); interval = setInterval(() => void status(),5000)
 void events(frame => {
  if (frame.startsWith('event: loss')) notice.value = 'Live events were dropped. Refresh traffic.'
  const line = frame.split('\n').find(x => x.startsWith('data:'))
  if (line) try {
   const data: unknown = JSON.parse(line.slice(5))
   if (data && typeof data === 'object' && 'kind' in data && data.kind === 'event_loss') notice.value = 'Inspection reported lost events. Refresh traffic for details.'
  } catch { /* Invalid metadata never enters the DOM. */ }
 },abort.signal)
})
onUnmounted(() => { mounted=false;clearUserDirectory();abort.abort(); clearInterval(interval) })
</script>
<template>
 <section id="console">
  <nav id="nav" aria-label="Console" class="mb-7 flex flex-wrap gap-2"><Button v-for="section in sections" :key="section.path" :variant="route.path===section.path?'secondary':'ghost'" :aria-current="String(route.path===section.path)" @click="router.push(section.path)">{{section.name}}</Button></nav>
  <p v-if="notice" role="status" class="mb-4 text-sm text-muted-foreground">{{notice}} <Button variant="ghost" size="sm" @click="notice=''">Dismiss</Button></p>
  <div id="page"><RouterView/></div>
 </section>
</template>

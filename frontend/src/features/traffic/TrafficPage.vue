<script setup lang="ts">
import { computed,onMounted,onUnmounted,ref,watch } from 'vue'
import type { ListResponse,Page } from '@/generated/contracts'
import { api,errorMessage,events,type LiveState } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Alert,AlertDescription } from '@/components/ui/alert'
import TrafficQuery from './TrafficQuery.vue'
import FlowConversation from './FlowConversation.vue'
import SessionList from './SessionList.vue'
import { flowRows,mergeFlows,text,type Flow } from './traffic'
import { trafficEvent,updateFlows } from './live'
import { parseEventFrame } from '@/shared/api/sse'
const rows=ref<Flow[]>([]),services=ref<{id:string;name:string;protocol_id:string}[]>([]),service=ref('all'),selected=ref<Flow>()
const cursor=ref<number|null>(),query=ref(''),loading=ref(false),error=ref(''),serviceError=ref(''),advanced=ref(false)
const liveState=ref<LiveState>('connecting'),liveNotice=ref(''),conversation=ref<InstanceType<typeof FlowConversation>>()
const selectedProtocol=computed(()=>services.value.find(s=>s.id===selected.value?.service_id)?.protocol_id??'raw')
const controller=new AbortController()
let request=0,catchup:ReturnType<typeof setTimeout>|undefined,disposed=false,loadAgain=false
function reconcile() {
 if(disposed)return
 if(loading.value){loadAgain=true;return}
 if(catchup)return
 catchup=setTimeout(()=>{catchup=undefined;void load(false,true);conversation.value?.resync()},100)
}
function receive(frame:string) {
 try {
  if(parseEventFrame(frame)?.event==='loss'){liveNotice.value='Some live events were lost. Reloading retained traffic.';reconcile();return}
  const event=trafficEvent(frame)
  if(!event)return
  if(event.kind==='event_loss'){liveNotice.value='Proxy reported telemetry loss. Some traffic may be unavailable.';reconcile();return}
  if(service.value!=='all'&&event.service_id!==service.value)return
  if(query.value){reconcile()}else if(event.kind==='flow_opened'||event.kind==='flow_closed') {
   if(event.kind==='flow_opened'&&event.transport!==1)return
   rows.value=updateFlows(rows.value,event)
   if(!selected.value&&rows.value.length)selected.value=rows.value[0]
   else if(selected.value)selected.value=rows.value.find(f=>f.flow_id===selected.value?.flow_id)??selected.value
  }
  if(event.flow_id===selected.value?.flow_id)conversation.value?.receive(event)
 } catch {liveNotice.value='Live event could not be read. Reloading retained traffic.';reconcile()}
}
async function load(more=false,live=false) {
 const current=++request;loading.value=true;error.value=''
 const parameters=new URLSearchParams(query.value)
 if(!parameters.has('transport'))parameters.set('transport','1')
 parameters.set('limit','100');parameters.set('newest_first','true')
 if(service.value!=='all')parameters.set('service_id',service.value)
 if(more&&cursor.value)parameters.set('after',String(cursor.value))
 try {
  const page=await api<Page>('/traffic/flows?'+parameters)
  if(disposed||current!==request)return
  const received=flowRows(page).map(flow=>{
   const old=rows.value.find(row=>row.flow_id===flow.flow_id)
   return old?.closed&&!flow.closed?{...flow,closed:old.closed,state:old.state,bytesUp:old.bytesUp,bytesDown:old.bytesDown}:flow
  })
  rows.value=mergeFlows(more||live?rows.value:[],received,more)
  if(!live||!cursor.value)cursor.value=page.next_cursor
  if(!selected.value&&rows.value.length)selected.value=rows.value[0]
  else if(selected.value)selected.value=rows.value.find(f=>f.flow_id===selected.value?.flow_id)??selected.value
 }catch(e){if(current===request)error.value=errorMessage(e)}
 finally{if(current===request){loading.value=false;if(loadAgain){loadAgain=false;reconcile()}}}
}
function search(value:string){query.value=value;selected.value=undefined;void load()}
watch(service,()=>{selected.value=undefined;rows.value=[];void load()})
onMounted(async()=>{
 try {
  let after=''
  do {
   const page=await api<ListResponse>('/services?limit=100'+(after?'&after='+encodeURIComponent(after):''))
   if(disposed)return
   services.value.push(...page.items.map(s=>({id:text(s.id),name:text(s.name)||text(s.id),protocol_id:text(s.protocol_id)||'raw'})))
   after=page.next_cursor??''
  }while(after&&services.value.length<128)
 }catch(e){serviceError.value=errorMessage(e)}
 if(disposed)return
 await load()
 if(!disposed)void events(receive,controller.signal,state=>{liveState.value=state;if(state==='live')reconcile()})
})
onUnmounted(()=>{disposed=true;request++;controller.abort();clearTimeout(catchup)})
</script>
<template>
 <section class="space-y-3">
  <div class="flex items-center justify-between"><div><h1 class="!mb-0">Traffic</h1><p id="traffic-live-status" role="status" class="text-xs text-muted-foreground">{{liveState==='live'?'Live':liveState==='retrying'?'Reconnecting live traffic…':'Connecting live traffic…'}}</p></div><Button variant="ghost" size="sm" :aria-expanded="advanced" @click="advanced=!advanced">{{advanced?'Hide filters':'Filters'}}</Button></div>
  <p v-if="liveNotice" role="status" class="text-xs text-muted-foreground">{{liveNotice}}</p>
  <TrafficQuery v-if="advanced" @search="search" />
  <Alert v-if="error||serviceError" variant="destructive"><AlertDescription>{{error||serviceError}}</AlertDescription></Alert>
  <div class="grid min-h-[640px] overflow-hidden rounded-md border bg-card md:h-[calc(100dvh-230px)] md:min-h-[480px] md:grid-cols-[300px_minmax(0,1fr)] lg:grid-cols-[330px_minmax(0,1fr)]">
   <SessionList :rows="rows" :services="services" :service="service" :selected="selected?.flow_id??''" :loading="loading" :more="Boolean(cursor)" @service="service=$event" @select="selected=$event" @refresh="load()" @more="load(true)" />
   <FlowConversation ref="conversation" v-if="selected" :key="selected.flow_id+':'+selectedProtocol" :flow-id="selected.flow_id" :flow="selected" :protocol-id="selectedProtocol" />
   <section v-else id="traffic-stream" class="flex min-h-80 items-center justify-center p-8 text-center"><div><h2 class="!mt-0">Follow TCP stream</h2><p class="text-sm text-muted-foreground">Choose a service and select a session to inspect its conversation.</p></div></section>
  </div>
 </section>
</template>

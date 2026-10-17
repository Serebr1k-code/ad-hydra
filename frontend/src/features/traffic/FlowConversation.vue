<script setup lang="ts">
import { computed,nextTick,onMounted,onUnmounted,ref } from 'vue'
import type { FlowHistory,ListResponse,Page } from '@/generated/contracts'
import { api,ApiError,errorMessage } from '@/shared/api/client'
import { openEditor } from '@/shared/editor'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Alert,AlertDescription } from '@/components/ui/alert'
import { Select,SelectContent,SelectItem,SelectTrigger,SelectValue } from '@/components/ui/select'
import FlowCapture from './FlowCapture.vue'
import { eventMessage,mergeMessages,type TrafficEvent } from './live'
import { differences,direction,fields,flowRows,historyPreview,historySegments,historyTextSegments,initialStreamMode,messageRows,record,streamText,timestamp,type Flow,type Message } from './traffic'
const props=defineProps<{flowId:string;flow?:Flow;protocolId?:string}>()
const detail=ref<Flow|undefined>(props.flow),messages=ref<Message[]>([]),cursor=ref<number|null>()
const inspect=ref<Message>(),selected=ref<Message>(),comparison=ref<ReturnType<typeof differences>>([]),comparisonTitle=ref('')
const streamBody=ref<HTMLElement>()
const rawProfile=computed(()=>props.protocolId==='raw')
const closureHistoryLost=computed(()=>record(detail.value?.closed).history_truncated===true)
const hasHistoryTextFallback=computed(()=>detail.value?.state==='Closed'&&messages.value.length===0&&historyState.value==='ready'&&history.value!==undefined)
const mode=ref(initialStreamMode(props.protocolId??'raw')),side=ref<'both'|'1'|'2'>('both'),loading=ref(false),error=ref(''),notice=ref('')
const byteMode=ref<'hex'|'utf8'|'ascii'>('hex'),history=ref<FlowHistory>(),historyState=ref<'idle'|'loading'|'ready'|'missing'|'active'|'unavailable'>('idle')
const visible=computed(()=>messages.value.filter(m=>side.value==='both'||String(m.direction)===side.value))
const path=`/traffic/flows/${encodeURIComponent(props.flowId)}`
let disposed=false,request=0,dirty=false
function receive(event:TrafficEvent) {
 if(event.flow_id!==props.flowId)return
 const message=eventMessage(event)
 const body=streamBody.value,follow=!body||body.scrollHeight-body.scrollTop-body.clientHeight<64
 if(message){messages.value=mergeMessages(messages.value,[message]);if(follow)void nextTick(()=>{if(streamBody.value)streamBody.value.scrollTop=streamBody.value.scrollHeight})}
 if(event.kind==='flow_closed'&&detail.value){detail.value={...detail.value,closed:event,state:'Closed',bytesUp:String(event.bytes_client_to_server??''),bytesDown:String(event.bytes_server_to_client??'')};void load()}
}
function resync(){if(loading.value){dirty=true;return}void load()}
defineExpose({receive,resync})
async function load(more=false) {
 const current=++request;loading.value=true;error.value=''
 try {
  if(more&&messages.value.length>=500)return
  const flow=await api<Record<string,unknown>>(path)
  if(disposed)return
  const received=flowRows({items:[flow],next_cursor:null})[0]
  if(!detail.value?.closed||received?.closed)detail.value=received
  if(received?.closed)void loadHistory()
  const page=await api<Page>(`${path}/messages?limit=100&newest_first=true${more&&cursor.value?'&after='+cursor.value:''}`)
  if(disposed||current!==request)return
  const initial=!messages.value.length
  messages.value=mergeMessages(messages.value,messageRows(page));cursor.value=page.next_cursor
  if(initial)void nextTick(()=>{if(streamBody.value)streamBody.value.scrollTop=streamBody.value.scrollHeight})
 }catch(e){if(!disposed&&current===request)error.value=errorMessage(e)}
 finally{if(current===request){loading.value=false;if(dirty&&!disposed){dirty=false;void load()}}}
}
async function loadHistory() {
 historyState.value='loading';history.value=undefined
 try {history.value=await api<FlowHistory>(`${path}/history`);historyState.value='ready'}
 catch(e) {historyState.value=e instanceof ApiError&&e.status===404?'missing':e instanceof ApiError&&e.status===409?'active':'unavailable'}
}
function compare(message:Message) {
 if(!selected.value){selected.value=message;comparison.value=[];comparisonTitle.value='';return}
 comparisonTitle.value=`${selected.value.message_id} ↔ ${message.message_id}`
 comparison.value=differences(selected.value,message);selected.value=undefined
}
function createFilter(message:Message,field:string) {
 openEditor('Create filter from saved field',{message_id:message.message_id,field,name:'Saved '+field,action:{type:'alert'}},async value=>{
  const filters=await api<ListResponse>('/filters')
  await api('/filters/from-message','POST',{...value,expected_revision:filters.revision})
  notice.value='Shadow filter saved. Test it on the saved message in Filters before enforcement.'
 })
}
onMounted(()=>{void load()})
onUnmounted(()=>{disposed=true;request++})
</script>
<template>
 <section id="traffic-stream" aria-label="Session stream" class="flex min-h-0 min-w-0 flex-col">
  <header class="space-y-3 border-b p-4">
   <div class="flex items-start justify-between gap-3"><div class="min-w-0"><h2 class="!m-0 text-base">Follow TCP stream</h2><p class="truncate font-mono text-xs text-muted-foreground">{{detail?.client}} ↔ {{detail?.destination}}</p></div><Button variant="ghost" size="sm" :disabled="loading" @click="load()">Refresh</Button></div>
   <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground"><span>{{detail?.service_id}}</span><span>· {{detail?.state??'Loading'}}</span><span v-if="detail?.bytesUp">· {{detail.bytesUp}} B sent / {{detail.bytesDown}} B received</span><span>· {{messages.length}} decoded messages</span></div>
   <div class="flex flex-wrap gap-2"><Select v-model="mode"><SelectTrigger aria-label="Stream view" class="w-48"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="decoded">Decoded conversation</SelectItem><SelectItem value="bytes">Captured bytes</SelectItem></SelectContent></Select><Select v-if="mode==='decoded'||detail?.state==='Closed'" v-model="side"><SelectTrigger aria-label="Stream direction" class="w-48"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="both">Both directions</SelectItem><SelectItem value="1">Client → server</SelectItem><SelectItem value="2">Server → client</SelectItem></SelectContent></Select><Select v-if="mode==='bytes'&&detail?.state==='Closed'" v-model="byteMode"><SelectTrigger aria-label="Raw byte format" class="w-36"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="hex">Hex dump</SelectItem><SelectItem value="utf8">UTF-8</SelectItem><SelectItem value="ascii">ASCII</SelectItem></SelectContent></Select></div>
   <p class="truncate text-[10px] text-muted-foreground" :title="flowId">Session {{flowId}} · {{detail?.started}}</p>
  </header>
  <div ref="streamBody" class="min-h-0 flex-1 space-y-3 overflow-auto p-4">
   <Alert v-if="error" variant="destructive"><AlertDescription>{{error}}</AlertDescription></Alert>
   <Alert v-if="notice"><AlertDescription>{{notice}}</AlertDescription></Alert>
   <FlowCapture v-if="mode==='bytes'&&detail?.state!=='Closed'" :flow-id="flowId" :live="true" :auto-capture="rawProfile" />
   <section v-if="mode==='bytes'&&detail?.state==='Closed'" class="space-y-3">
    <p v-if="historyState==='loading'" role="status" class="text-sm text-muted-foreground">Loading saved stream history…</p>
    <Alert v-else-if="historyState==='missing'" :variant="closureHistoryLost?'destructive':'default'"><AlertDescription>{{closureHistoryLost?'No history record was saved; capture capacity or storage limits caused history loss.':'No saved history is available. Retention may have been disabled, or the record may have expired or been evicted.'}}</AlertDescription></Alert>
    <Alert v-else-if="historyState==='active'"><AlertDescription>The stream is still active; refresh after it closes to load its saved history.</AlertDescription></Alert>
    <Alert v-else-if="historyState==='unavailable'" variant="destructive"><AlertDescription>Saved stream history is temporarily unavailable. Refresh to retry.</AlertDescription></Alert>
    <template v-if="historyState==='ready'&&history">
     <p class="text-xs text-muted-foreground">{{history.segments.reduce((n,s)=>n+s.data.length,0)}} retained bytes · {{history.bytes_client_to_server}} client → server / {{history.bytes_server_to_client}} server → client</p>
     <Alert v-if="history.history_truncated||history.truncated_client_to_server||history.truncated_server_to_client"><AlertDescription>History is truncated. Only the latest bounded bytes are retained; some data may have been lost.</AlertDescription></Alert>
     <p v-if="!history.segments.length" class="text-sm text-muted-foreground">The flow closed without any retained payload bytes.</p>
     <article v-for="(segment,index) in historySegments(history,side)" :key="index" class="space-y-1 border-l-2 pl-3" :class="segment.direction===1?'border-red-400/60':'border-blue-400/60'">
      <div class="text-[11px] text-muted-foreground">{{direction(segment.direction)}} · {{segment.data.length}} bytes<span v-if="segment.truncated"> · direction tail truncated</span></div>
      <pre class="!max-h-none !rounded-none !border-0 !bg-transparent !p-0 font-mono !text-[13px] !leading-6">{{historyPreview(segment,byteMode)||'(empty)'}}</pre>
     </article>
    </template>
   </section>
   <template v-if="mode==='decoded'">
    <p v-if="messages.length>=500" class="text-xs text-muted-foreground">Showing the latest 500 decoded messages in this session.</p>
    <p class="text-xs text-muted-foreground">Decoded fields in session order. Client traffic is red; server traffic is blue. Byte samples are available under Captured bytes.</p>
    <p v-if="loading&&!messages.length" role="status">Loading conversation…</p>
    <template v-if="hasHistoryTextFallback&&history">
     <p class="text-xs text-muted-foreground">No parser messages were retained. Showing saved stream bytes as UTF-8, grouped by direction; packet timing and protocol fields are unavailable. Switch to Captured bytes for the default hex view.</p>
     <article v-for="(segment,index) in historyTextSegments(history,side)" :key="index" class="space-y-1 border-l-2 pl-3" :class="segment.direction===1?'border-red-400/60':'border-blue-400/60'">
      <div class="text-[11px] text-muted-foreground">{{direction(segment.direction)}} · {{segment.data.length}} retained bytes<span v-if="segment.truncated"> · direction tail truncated</span></div>
      <pre class="!max-h-none !rounded-none !border-0 !bg-transparent !p-0 font-mono !text-[13px] !leading-6">{{historyPreview(segment,'utf8')||'(empty)'}}</pre>
     </article>
    </template>
    <p v-else-if="!loading&&!messages.length" class="py-8 text-sm text-muted-foreground">No decoded messages were retained for this session. Unknown and encrypted protocols still have session metadata. Closed streams appear here as UTF-8 when saved history is available; use Captured bytes to inspect the next chunk on a live session.</p>
    <article v-for="message in visible" :key="message.message_id" :data-message-id="message.message_id" :data-direction="message.direction" class="border-l-2 pl-3" :class="message.direction===1?'border-red-400/60':message.direction===2?'border-blue-400/60':'border-border'">
     <div class="mb-1 flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground"><span :class="message.direction===1?'text-red-300':message.direction===2?'text-blue-300':''">{{direction(message.direction)}}</span><span>{{timestamp(message.occurred_ms)}}</span><span>{{message.message_type||message.protocol_id}}</span><span v-if="message.truncated">Truncated</span></div>
     <pre class="!max-h-none !rounded-none !border-0 !bg-transparent !p-0 font-mono !text-[13px] !leading-6">{{streamText(message)}}</pre>
     <div class="flex gap-2"><Button size="sm" variant="ghost" @click="inspect=inspect?.message_id===message.message_id?undefined:message">{{inspect?.message_id===message.message_id?'Hide fields':'Inspect fields'}}</Button><Button size="sm" variant="ghost" @click="compare(message)">Compare</Button></div>
     <DataTable v-if="inspect?.message_id===message.message_id" :columns="[{key:'key',label:'Field'},{key:'type',label:'Type'},{key:'value',label:'Value'},{key:'filter',label:'Filter'}]" :rows="fields(message)"><template #cell-filter="{row}"><Button size="sm" variant="outline" @click="createFilter(message,row.key)">Create shadow filter</Button></template></DataTable>
    </article>
    <p v-if="selected" class="text-xs text-muted-foreground">Choose another message to compare with {{selected.message_id}}.</p>
    <section v-if="comparisonTitle"><h3 class="text-xs">{{comparisonTitle}}</h3><DataTable :columns="[{key:'key',label:'Field'},{key:'left',label:'Selected'},{key:'right',label:'Current'},{key:'change',label:'Difference'}]" :rows="comparison" /></section>
    <Button v-if="cursor" variant="outline" :disabled="loading||messages.length>=500" @click="load(true)">More messages</Button>
    <p v-if="messages.length>=500" class="text-xs text-muted-foreground">500-message preview limit reached.</p>
   </template>
  </div>
 </section>
</template>

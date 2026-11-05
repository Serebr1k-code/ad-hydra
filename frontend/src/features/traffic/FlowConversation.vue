<script setup lang="ts">
import { computed,defineAsyncComponent,nextTick,onMounted,onUnmounted,ref,watch } from 'vue'
import type { FlowHistory,ListResponse,Page } from '@/generated/contracts'
import { api,ApiError,errorMessage } from '@/shared/api/client'
import { openEditor } from '@/shared/editor'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Alert,AlertDescription } from '@/components/ui/alert'
import { Select,SelectContent,SelectItem,SelectTrigger,SelectValue } from '@/components/ui/select'
import FlowCapture from './FlowCapture.vue'
import HttpMessage from './HttpMessage.vue'
import {isHttpMessage} from './http'
import {terminalMessages,terminalHistory} from './terminal'
const TerminalStream=defineAsyncComponent(()=>import('./TerminalStream.vue'))
import FlowFavoriteButton, { type FavoriteChange } from './FlowFavoriteButton.vue'
import { eventMessage,mergeMessages,type TrafficEvent } from './live'
import { canReadSavedHistory,differences,direction,fields,flowRows,historyPreview,historySegments,historyTextSegments,initialStreamMode,messageRows,record,streamText,timestamp,type Flow,type Message } from './traffic'
const props=defineProps<{flowId:string;flow?:Flow;protocolId?:string;showBlocked?:boolean}>()
const emit=defineEmits<{favorite:[value:FavoriteChange]}>()
const detail=ref<Flow|undefined>(props.flow),messages=ref<Message[]>([]),cursor=ref<number|null>()
const inspect=ref<Message>(),selected=ref<Message>(),comparison=ref<ReturnType<typeof differences>>([]),comparisonTitle=ref('')
const streamBody=ref<HTMLElement>()
const udp=computed(()=>detail.value?.transport===2)
const rawProfile=computed(()=>props.protocolId==='raw')
const closureHistoryLost=computed(()=>record(detail.value?.closed).history_truncated===true)
const savedStream=computed(()=>canReadSavedHistory(detail.value))
const hasHistoryTextFallback=computed(()=>savedStream.value&&messages.value.length===0&&historyState.value==='ready'&&history.value!==undefined)
const mode=ref(initialStreamMode(props.protocolId??'raw')),side=ref<'both'|'1'|'2'>('both'),loading=ref(false),error=ref(''),notice=ref('')
const byteMode=ref<'hex'|'utf8'|'ascii'>('hex'),history=ref<FlowHistory>(),historyState=ref<'idle'|'loading'|'ready'|'missing'|'active'|'unavailable'>('idle')
const visible=computed(()=>messages.value.filter(m=>(props.showBlocked||m.blocked!==true)&&(side.value==='both'||String(m.direction)===side.value)))
const terminalTranscript=computed(()=>{
 if(history.value){const saved=terminalHistory(history.value.segments,side.value);if(saved.ordered||side.value!=='both')return saved}
 return terminalMessages(visible.value,side.value)
})
const terminalLegacyOrder=computed(()=>historyState.value==='ready'&&history.value&&history.value.segments.filter(s=>s.data.length>0).length>1&&!history.value.segments.filter(s=>s.data.length>0).every(s=>s.sequence>0)&&!terminalTranscript.value.chunks.length&&side.value==='both')
const path=`/traffic/flows/${encodeURIComponent(props.flowId)}`
function favoriteChanged(change:FavoriteChange){if(detail.value)detail.value={...detail.value,...change};if(savedStream.value)void loadHistory();emit('favorite',change)}
watch(()=>props.flow,value=>{
 if(detail.value&&value){
  const wasSaved=savedStream.value
  detail.value={...detail.value,favorite:value.favorite,raw_history_state:value.raw_history_state,decoded_incomplete:value.decoded_incomplete}
  if(!wasSaved&&savedStream.value)void loadHistory()
 }
})
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
  if(canReadSavedHistory(received))void loadHistory()
  const page=await api<Page>(`${path}/messages?limit=100&include_blocked=${props.showBlocked===true}&newest_first=true${more&&cursor.value?'&after='+cursor.value:''}`)
  if(disposed||current!==request)return
  const initial=!messages.value.length
  messages.value=mergeMessages(messages.value,messageRows(page));cursor.value=page.next_cursor
  if(initial)void nextTick(()=>{if(streamBody.value)streamBody.value.scrollTop=streamBody.value.scrollHeight})
 }catch(e){if(!disposed&&current===request)error.value=errorMessage(e)}
 finally{if(current===request){loading.value=false;if(dirty&&!disposed){dirty=false;void load()}}}
}
async function loadHistory() {
 if(historyState.value==='loading')return
 historyState.value='loading'
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
  notice.value='Filter saved in Enforce mode. Check the applied revision in Filters.'
 })
}
onMounted(()=>{void load()})
onUnmounted(()=>{disposed=true;request++})
</script>
<template>
 <section id="traffic-stream" aria-label="Session stream" class="@container flex min-h-0 min-w-0 flex-col">
  <header class="space-y-3 border-b p-4">
   <div class="flex items-start justify-between gap-3"><div class="min-w-0"><h2 class="!m-0 text-base">{{udp?'Follow UDP conversation':'Follow TCP stream'}}</h2><p class="truncate font-mono text-xs text-muted-foreground">{{detail?.client}} ↔ {{detail?.destination}}</p></div><div class="flex flex-wrap justify-end gap-1"><FlowFavoriteButton v-if="detail" :flow-id="flowId" :favorite="detail.favorite" @changed="favoriteChanged" @error="error=$event" /><Button variant="ghost" size="sm" :disabled="loading" @click="load()">Refresh</Button></div></div>
   <p v-if="detail?.decoded_incomplete&&!detail.favorite" role="status" class="text-xs text-muted-foreground">Decoded history is partial; older data was rotated at this service’s retention limit.</p>
   <p v-if="detail?.favorite" class="text-xs text-muted-foreground">Saved to favourites; ordinary traffic rotation does not remove this stream.<span v-if="detail?.raw_history_state==='pending'"> Capture is pending completion; the saved snapshot is retained.</span><span v-if="detail?.raw_history_state==='partial'"> The connection was interrupted; its saved snapshot is partial and remains available under Captured bytes.</span><span v-if="detail?.raw_history_state==='unavailable'"> Captured bytes were unavailable when saved.</span><span v-if="detail?.decoded_incomplete"> Decoded history is incomplete because a capture limit was reached.</span></p>
   <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground"><span>{{detail?.service_id}}</span><span v-if="flow?.has_flag||detail?.has_flag" class="rounded border border-amber-500/60 bg-amber-500/10 px-2 py-1 font-medium text-amber-500">Flag detected</span><span v-if="flow?.blocked||detail?.blocked" class="text-destructive">Blocked</span><span>· {{detail?.state??'Loading'}}</span><span v-if="detail?.bytesUp">· {{detail.bytesUp}} B sent / {{detail.bytesDown}} B received</span><span>· {{messages.length}} decoded messages</span></div>
   <div class="flex flex-wrap gap-2"><Select v-model="mode"><SelectTrigger aria-label="Stream view" class="w-48"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="decoded">Decoded conversation</SelectItem><SelectItem value="bytes">Captured bytes</SelectItem><SelectItem value="terminal">Stream · terminal</SelectItem></SelectContent></Select><Select v-if="mode==='decoded'||mode==='terminal'||savedStream" v-model="side"><SelectTrigger aria-label="Stream direction" class="w-48"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="both">Both directions</SelectItem><SelectItem value="1">Client → server</SelectItem><SelectItem value="2">Server → client</SelectItem></SelectContent></Select><Select v-if="mode==='bytes'&&savedStream" v-model="byteMode"><SelectTrigger aria-label="Raw byte format" class="w-36"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="hex">Hex dump</SelectItem><SelectItem value="utf8">UTF-8</SelectItem><SelectItem value="ascii">ASCII</SelectItem></SelectContent></Select></div>
   <p class="truncate text-[10px] text-muted-foreground" :title="flowId">Session {{flowId}} · {{detail?.started}}</p>
  </header>
  <div ref="streamBody" class="min-h-0 flex-1" :class="mode==='terminal'?'flex flex-col gap-3 overflow-hidden p-3':'space-y-3 overflow-auto p-4'">
   <Alert v-if="error" variant="destructive"><AlertDescription>{{error}}</AlertDescription></Alert>
   <Alert v-if="notice"><AlertDescription>{{notice}}</AlertDescription></Alert>
   <template v-if="mode==='terminal'">
    <p v-if="(loading||historyState==='loading')&&!terminalTranscript.chunks.length" role="status">Loading terminal stream…</p>
    <p v-if="terminalLegacyOrder" class="text-sm text-muted-foreground">This older capture saved separate direction buffers without conversation order. Select Client or Server to inspect it in a terminal.</p>
    <p v-else-if="!loading&&historyState!=='loading'&&!terminalTranscript.chunks.length" class="text-sm text-muted-foreground">{{savedStream?'No retained text is available for this session. Capture history may have expired or been disabled.':protocolId==='utf8'?'Waiting for captured UTF-8 text from either direction…':'Live terminal playback requires the UTF-8 decoder. Saved bytes can be replayed after the session closes.'}}</p>
    <p v-if="history?.history_truncated||detail?.decoded_incomplete" class="text-xs text-muted-foreground">Capture history is incomplete; only retained text can be replayed.</p>
    <TerminalStream :chunks="terminalTranscript.chunks" :truncated="terminalTranscript.truncated" />
   </template>
   <FlowCapture v-if="mode==='bytes'&&!savedStream" :flow-id="flowId" :live="true" :auto-capture="rawProfile" />
   <section v-if="mode==='bytes'&&savedStream" class="space-y-3">
    <p v-if="historyState==='loading'" role="status" class="text-sm text-muted-foreground">Loading saved stream history…</p>
    <Alert v-else-if="historyState==='missing'" :variant="closureHistoryLost?'destructive':'default'"><AlertDescription>{{closureHistoryLost?'No history record was saved; capture capacity or storage limits caused history loss.':'No saved history is available. Retention may have been disabled, or the record may have expired or been evicted.'}}</AlertDescription></Alert>
    <Alert v-else-if="historyState==='active'"><AlertDescription>The stream is still active; refresh after it closes to load its saved history.</AlertDescription></Alert>
    <Alert v-else-if="historyState==='unavailable'" variant="destructive"><AlertDescription>Saved stream history is temporarily unavailable. Refresh to retry.</AlertDescription></Alert>
    <template v-if="historyState==='ready'&&history">
     <p class="text-xs text-muted-foreground">{{history.segments.reduce((n,s)=>n+s.data.length,0)}} retained bytes<span v-if="udp"> in {{history.segments.length}} datagrams</span><template v-if="history.closed_at_ms!==0"> · {{history.bytes_client_to_server}} bytes forwarded client → server / {{history.bytes_server_to_client}} server → client</template><span v-else> · Final forwarded totals are unavailable for this interrupted snapshot.</span></p>
     <p class="text-xs text-muted-foreground">Captured before rule enforcement; retained bytes may include blocked traffic. Forwarded byte totals exclude blocked traffic.</p>
     <Alert v-if="history.history_truncated||history.truncated_client_to_server||history.truncated_server_to_client"><AlertDescription>History is truncated. Only the latest bounded bytes are retained; some data may have been lost.</AlertDescription></Alert>
     <p v-if="!history.segments.length" class="text-sm text-muted-foreground">The flow closed without any retained payload bytes.</p>
     <article v-for="(segment,index) in historySegments(history,side)" :key="index"  :data-direction="segment.direction" class="space-y-1 rounded-sm border-l-2 p-3 @3xl:w-[78%]" :class="segment.direction===1?'border-red-400/60 bg-red-500/5 @3xl:mr-auto':'border-blue-400/60 bg-blue-500/5 @3xl:ml-auto'">
      <div class="text-[11px] text-muted-foreground">{{direction(segment.direction)}} · {{segment.data.length}} bytes<span v-if="segment.datagram"> · datagram #{{segment.sequence}} · {{timestamp(segment.occurred_at_ms)}}</span><span v-if="segment.truncated"> · direction tail truncated</span></div>
      <pre class="!max-h-none !rounded-none !border-0 !bg-transparent !p-0 font-mono !text-[13px] !leading-6">{{historyPreview(segment,byteMode)||'(empty)'}}</pre>
     </article>
    </template>
   </section>
   <template v-if="mode==='decoded'">
    <p v-if="messages.length>=500" class="text-xs text-muted-foreground">Showing the latest 500 decoded messages in this session.</p>
    <p class="text-xs text-muted-foreground">Decoded fields in session order. Client traffic is red; server traffic is blue. Byte samples are available under Captured bytes.</p>
    <p v-if="loading&&!messages.length" role="status">Loading conversation…</p>
    <template v-if="hasHistoryTextFallback&&history">
     <p class="text-xs text-muted-foreground">No parser messages were retained. Showing saved bytes as UTF-8. UDP datagrams retain their boundaries and arrival order; TCP bytes are grouped by direction. Switch to Captured bytes for the default hex view.</p>
     <article v-for="(segment,index) in historyTextSegments(history,side)" :key="index"  :data-direction="segment.direction" class="space-y-1 rounded-sm border-l-2 p-3 @3xl:w-[78%]" :class="segment.direction===1?'border-red-400/60 bg-red-500/5 @3xl:mr-auto':'border-blue-400/60 bg-blue-500/5 @3xl:ml-auto'">
      <div class="text-[11px] text-muted-foreground">{{direction(segment.direction)}} · {{segment.data.length}} retained bytes<span v-if="segment.datagram"> · datagram #{{segment.sequence}} · {{timestamp(segment.occurred_at_ms)}}</span><span v-if="segment.truncated"> · direction tail truncated</span></div>
      <pre class="!max-h-none !rounded-none !border-0 !bg-transparent !p-0 font-mono !text-[13px] !leading-6">{{historyPreview(segment,'utf8')||'(empty)'}}</pre>
     </article>
    </template>
    <p v-else-if="!loading&&!messages.length" class="py-8 text-sm text-muted-foreground">No decoded messages were retained for this session. Unknown and encrypted protocols still have session metadata. Closed conversations appear here as UTF-8 when saved history is available; use Captured bytes for hex or to inspect the next chunk on a live session.</p>
    <article v-for="message in visible" :key="message.message_id" :data-message-id="message.message_id" :data-direction="message.direction" class="rounded-sm border-l-2 p-3 @3xl:w-[78%]" :class="message.direction===1?'border-red-400/60 bg-red-500/5 @3xl:mr-auto':message.direction===2?'border-blue-400/60 bg-blue-500/5 @3xl:ml-auto':'border-border'">
     <div class="mb-1 flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground"><span :class="message.direction===1?'text-red-300':message.direction===2?'text-blue-300':''">{{direction(message.direction)}}</span><span>{{timestamp(message.occurred_ms)}}</span><span>{{message.message_type||message.protocol_id}}</span><span v-if="message.truncated">Truncated</span></div>
     <HttpMessage v-if="isHttpMessage(message)" :message="message" @bytes="mode='bytes'" />
     <pre v-else class="!max-h-none !rounded-none !border-0 !bg-transparent !p-0 font-mono !text-[13px] !leading-6">{{streamText(message)}}</pre>
     <div class="flex gap-2"><Button size="sm" variant="ghost" @click="inspect=inspect?.message_id===message.message_id?undefined:message">{{inspect?.message_id===message.message_id?'Hide fields':'Inspect fields'}}</Button><Button size="sm" variant="ghost" @click="compare(message)">Compare</Button></div>
     <DataTable v-if="inspect?.message_id===message.message_id" :columns="[{key:'key',label:'Field'},{key:'type',label:'Type'},{key:'value',label:'Value'},{key:'filter',label:'Filter'}]" :rows="fields(message)"><template #cell-filter="{row}"><Button size="sm" variant="outline" @click="createFilter(message,row.key)">Create filter</Button></template></DataTable>
    </article>
    <p v-if="selected" class="text-xs text-muted-foreground">Choose another message to compare with {{selected.message_id}}.</p>
    <section v-if="comparisonTitle"><h3 class="text-xs">{{comparisonTitle}}</h3><DataTable :columns="[{key:'key',label:'Field'},{key:'left',label:'Selected'},{key:'right',label:'Current'},{key:'change',label:'Difference'}]" :rows="comparison" /></section>
    <Button v-if="cursor" variant="outline" :disabled="loading||messages.length>=500" @click="load(true)">More messages</Button>
    <p v-if="messages.length>=500" class="text-xs text-muted-foreground">500-message preview limit reached.</p>
   </template>
  </div>
 </section>
</template>

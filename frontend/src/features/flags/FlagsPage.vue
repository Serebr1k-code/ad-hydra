<script setup lang="ts">
import { computed,onMounted,onUnmounted,ref,watch } from 'vue'
import type { FlagPage,FlagView,SubmissionView,Triage,ListResponse } from '@/generated/contracts'
import { api,errorMessage } from '@/shared/api/client'
import { date } from '@/shared/format'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select,SelectContent,SelectItem,SelectTrigger,SelectValue } from '@/components/ui/select'
import CapturedFlagForm from './CapturedFlagForm.vue'
import FlagRecords from './FlagRecords.vue'
import SubmitterForm from './SubmitterForm.vue'
const pageData=ref<FlagPage>(),submissions=ref<SubmissionView[]>([]),services=ref<{id:string;name:string}[]>([])
const offset=ref(0),pageSize=ref('50'),sort=ref('created_at'),order=ref('desc'),service=ref('all'),state=ref('all'),query=ref(''),search=ref(''),target=ref(''),submissionOffset=ref(0)
const loading=ref(true),busy=ref(false),error=ref(''),notice=ref('')
const columns=[{key:'flag_id',label:'Flag ID'},{key:'state',label:'State'},{key:'attempts',label:'Attempts'},{key:'initiated_by',label:'Initiated by'},{key:'created_at',label:'Created'},{key:'response',label:'Checker outcome'}]
const serviceNames=computed(()=>Object.fromEntries(services.value.map(item=>[item.id,item.name])))
const suffix=computed(()=>`?offset=${offset.value}&limit=${pageSize.value}&search=${encodeURIComponent(search.value)}&sort=${sort.value}&order=${order.value}${service.value==='all'?'':'&service_id='+encodeURIComponent(service.value)}${state.value==='all'?'':'&state='+state.value}`)
let mounted=true,polling=false,timer:ReturnType<typeof setInterval>|undefined,request=0
async function load(silent=false){
 const current=++request;if(!silent)loading.value=true
 try{
  const result=await Promise.all([api<FlagPage>('/flags/page'+suffix.value),api<SubmissionView[]>(`/submissions?offset=${submissionOffset.value}&limit=100`)])
  if(mounted&&current===request)[pageData.value,submissions.value]=result
 }finally{if(current===request)loading.value=false}
}
async function action(work:()=>Promise<unknown>,success=''){
 if(busy.value)return
 busy.value=true;error.value='';notice.value=''
 try{await work();notice.value=success}catch(cause){error.value=errorMessage(cause)}finally{busy.value=false}
}
function refresh(){void action(()=>load())}
function find(){search.value=query.value;offset.value=0;refresh()}
function submit(flag:FlagView){void action(async()=>{await api(`/flags/${encodeURIComponent(flag.id)}/submit`,'POST');await load()},'Submission queued.')}
function triage(flag:FlagView,resolution:string){void action(async()=>{
 const body:Triage={resolution}
 if(resolution==='captured'){if(!target.value)throw new Error('Enter an enabled authorized target ID.');body.target_id=target.value}
 await api(`/flags/${encodeURIComponent(flag.id)}/triage`,'POST',body);await load()
},'Triage saved.')}
async function poll(){if(!mounted||polling||busy.value)return;polling=true;try{await load(true)}catch(cause){if(mounted)error.value=errorMessage(cause)}finally{polling=false}}
watch([sort,order,service,state,pageSize],()=>{offset.value=0;refresh()})
onMounted(async()=>{
 try{const result=await api<ListResponse>('/farm/services');services.value=result.items.filter(item=>item.enabled!==false).map(item=>({id:String(item.id),name:String(item.name??item.id)}))}catch(cause){error.value=errorMessage(cause)}
 refresh();timer=setInterval(()=>{void poll()},3000)
})
onUnmounted(()=>{mounted=false;request++;if(timer)clearInterval(timer)})
</script>
<template>
 <div class="space-y-6">
  <header><h1 class="text-2xl font-semibold">Flag Manager</h1><p class="text-muted-foreground">Captured flags are queued unchanged. Defensive observations keep stream markers only; their flag values are not stored in the vault.</p></header>
  <p v-if="error" role="alert" class="text-destructive">{{error}}</p><p v-if="notice" role="status">{{notice}}</p>
  <SubmitterForm @saved="refresh" />
  <CapturedFlagForm v-model:target="target" :services="services" @saved="refresh" />
  <form class="flex flex-wrap items-end gap-3" @submit.prevent="find"><div class="min-w-64 flex-1"><Label for="flag-search">Search ID, digest, category, service or target</Label><Input id="flag-search" v-model="query" maxlength="128" :disabled="busy" /></div><Button type="submit" :disabled="busy">Search</Button><Button type="button" variant="outline" :disabled="busy" @click="refresh">Refresh</Button></form>
  <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
   <div><Label>Service</Label><Select v-model="service" :disabled="busy"><SelectTrigger aria-label="Filter flags by service"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">All services</SelectItem><SelectItem v-for="item in services" :key="item.id" :value="item.id">{{item.name}}</SelectItem></SelectContent></Select></div>
   <div><Label>State</Label><Select v-model="state" :disabled="busy"><SelectTrigger aria-label="Filter flags by state"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">All states</SelectItem><SelectItem v-for="value in ['queued','inflight','retry','accepted','failed','quarantined']" :key="value" :value="value">{{value}}</SelectItem></SelectContent></Select></div>
   <div><Label>Sort</Label><Select v-model="sort" :disabled="busy"><SelectTrigger aria-label="Sort flags"><SelectValue /></SelectTrigger><SelectContent><SelectItem v-for="value in ['created_at','service','target','state','category','digest']" :key="value" :value="value">{{value}}</SelectItem></SelectContent></Select></div>
   <div><Label>Order</Label><Select v-model="order" :disabled="busy"><SelectTrigger aria-label="Flag sort order"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="desc">Descending</SelectItem><SelectItem value="asc">Ascending</SelectItem></SelectContent></Select></div>
   <div><Label>Page size</Label><Select v-model="pageSize" :disabled="busy"><SelectTrigger aria-label="Flags per page"><SelectValue /></SelectTrigger><SelectContent><SelectItem v-for="value in ['25','50','100','200']" :key="value" :value="value">{{value}}</SelectItem></SelectContent></Select></div>
  </div>
  <div class="flex flex-wrap items-center gap-3"><Button variant="outline" :disabled="busy||offset===0" @click="offset=Math.max(0,offset-Number(pageSize));refresh()">Previous</Button><Button variant="outline" :disabled="busy||offset+Number(pageSize)>=(pageData?.total??0)" @click="offset+=Number(pageSize);refresh()">Next</Button><p class="text-sm text-muted-foreground">Page {{Math.floor(offset/Number(pageSize))+1}} · {{pageData?.total??0}} total flags</p></div>
  <p v-if="loading" role="status">Loading retained records…</p>
  <FlagRecords :flags="pageData?.items??[]" :service-names="serviceNames" :busy="busy" @submit="submit" @triage="triage" />
  <section class="space-y-3"><h2 class="text-lg font-semibold">Submissions</h2><DataTable :columns="columns" :rows="submissions"><template #cell-created_at="{row}">{{date(row.created_at)}}</template></DataTable><div class="flex gap-2"><Button variant="outline" :disabled="busy||!submissionOffset" @click="submissionOffset=Math.max(0,submissionOffset-100);refresh()">Previous submissions</Button><Button variant="outline" :disabled="busy||submissions.length<100" @click="submissionOffset+=100;refresh()">Next submissions</Button></div></section>
 </div>
</template>

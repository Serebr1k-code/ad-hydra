<script setup lang="ts">
import {onMounted,onUnmounted,ref} from 'vue'
import type {SubmitterBatch,SubmitterBatchPage,SubmitterBatchSummary} from '@/generated/contracts'
import {api,errorMessage} from '@/shared/api/client'
import {date} from '@/shared/format'
import {Button} from '@/components/ui/button'
import DataTable from '@/components/DataTable.vue'
const page=ref<SubmitterBatchPage>(),selected=ref<SubmitterBatch>(),selectedId=ref(''),offset=ref(0),busy=ref(false),error=ref('')
const columns=[{key:'started_at',label:'Started'},{key:'status',label:'Result'},{key:'flag_count',label:'Flags'},{key:'exit_code',label:'Exit code'},{key:'actions',label:'Details'}]
let mounted=true,timer:ReturnType<typeof setInterval>|undefined,loading=false,sequence=0
async function load(){
 if(loading)return
 loading=true;const request=++sequence
 try{
  const next=await api<SubmitterBatchPage>(`/submitter/batches?offset=${offset.value}&limit=20`)
  if(!mounted||request!==sequence)return
  page.value=next
  if(!selectedId.value&&next.items[0])selectedId.value=next.items[0].id
  if(selectedId.value){const id=selectedId.value;const detail=await api<SubmitterBatch>(`/submitter/batches/${encodeURIComponent(id)}`);if(mounted&&request===sequence&&selectedId.value===id)selected.value=detail}
  error.value=''
 }catch(cause){if(mounted)error.value=errorMessage(cause)}finally{loading=false;busy.value=false}
}
async function open(row:SubmitterBatchSummary){selectedId.value=row.id;selected.value=undefined;busy.value=true;try{const detail=await api<SubmitterBatch>(`/submitter/batches/${encodeURIComponent(row.id)}`);if(mounted&&selectedId.value===row.id)selected.value=detail}catch(cause){error.value=errorMessage(cause)}finally{busy.value=false}}
function turn(delta:number){offset.value=Math.max(0,offset.value+delta);void load()}
onMounted(()=>{void load();timer=setInterval(()=>{void load()},4000)})
onUnmounted(()=>{mounted=false;sequence++;if(timer)clearInterval(timer)})
</script>
<template>
 <section class="space-y-3 border-t pt-4">
  <div class="flex items-center justify-between"><h3 class="font-medium">Submission attempts</h3><Button variant="outline" :disabled="busy" @click="load">Refresh attempts</Button></div>
  <p v-if="error" role="alert" class="text-destructive">{{error}}</p>
  <DataTable :columns="columns" :rows="page?.items??[]"><template #cell-started_at="{row}">{{date(row.started_at)}}</template><template #cell-actions="{row}"><Button variant="outline" :disabled="busy" @click="open(row)">Input / output</Button></template></DataTable>
  <div class="flex items-center gap-3"><Button variant="outline" :disabled="busy||offset===0" @click="turn(-20)">Previous attempts</Button><Button variant="outline" :disabled="busy||offset+20>=(page?.total??0)" @click="turn(20)">Next attempts</Button><span class="text-sm">{{page?.total??0}} attempts</span></div>
  <section v-if="selected" class="space-y-3 rounded border p-3" aria-live="polite">
   <h4 class="font-medium">Attempt {{selected.id}} · {{selected.status}} · {{selected.flag_count}} flags</h4>
   <div><h5 class="text-sm font-medium">Input flags (exact arguments)</h5><pre class="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded border p-3 text-sm">{{selected.input_flags===null?'Input was not retained for this historical attempt.':JSON.stringify(selected.input_flags,null,2)}}</pre></div>
   <div><h5 class="text-sm font-medium">Standard output</h5><pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded border p-3 text-sm">{{selected.stdout||'(No standard output)'}}</pre></div>
   <div><h5 class="text-sm font-medium">Standard error</h5><pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded border p-3 text-sm">{{selected.stderr||'(No standard error)'}}</pre></div>
  </section>
 </section>
</template>

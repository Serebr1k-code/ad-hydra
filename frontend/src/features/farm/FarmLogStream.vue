<script setup lang="ts">
import {onMounted,onUnmounted,ref,watch} from 'vue'
import {api,errorMessage} from '@/shared/api/client'
import {Button} from '@/components/ui/button'
import type {FarmLogPage} from './types'
const props=defineProps<{runId:string;stream:'stdout'|'stderr'}>()
const page=ref<FarmLogPage>(),offset=ref(0),follow=ref(true),busy=ref(false),error=ref('')
let mounted=true,timer:ReturnType<typeof setInterval>|undefined,sequence=0
async function load(){
 if(busy.value)return
 busy.value=true;const request=++sequence
 try{
  const path=`/farm/runs/${encodeURIComponent(props.runId)}/logs?stream=${props.stream}&limit=65536&offset=`
  let requested=offset.value
  let result=await api<FarmLogPage>(path+requested)
  if(follow.value&&result.end_offset-result.start_offset>65536){requested=Math.max(result.start_offset,result.end_offset-65536);result=await api<FarmLogPage>(path+requested)}
  if(!mounted||request!==sequence)return
  page.value=result;offset.value=Math.max(result.start_offset,requested);error.value=''
 }catch(cause){if(mounted)error.value=errorMessage(cause)}finally{if(request===sequence)busy.value=false}
}
function move(next:number){follow.value=false;offset.value=Math.max(0,next);void load()}
function tail(){follow.value=true;void load()}
watch(()=>props.runId,()=>{sequence++;offset.value=0;page.value=undefined;follow.value=true;busy.value=false;void load()})
onMounted(()=>{void load();timer=setInterval(()=>{if(follow.value)void load()},2000)})
onUnmounted(()=>{mounted=false;sequence++;if(timer)clearInterval(timer)})
</script>
<template>
 <section class="space-y-2" :data-log-stream="stream">
  <h3 class="font-medium">{{stream==='stdout'?'Standard output':'Standard error'}}</h3>
  <p v-if="error" role="alert" class="text-destructive">{{error}}</p>
  <div class="flex flex-wrap items-center gap-2 text-sm">
   <Button variant="outline" :disabled="busy||offset<=(page?.start_offset??0)" @click="move(page?.start_offset??0)">First retained</Button>
   <Button variant="outline" :disabled="busy||offset<=(page?.start_offset??0)" @click="move(Math.max(page?.start_offset??0,offset-65536))">Previous output</Button>
   <Button variant="outline" :disabled="busy||(page?.next_offset??0)>=(page?.end_offset??0)" @click="move(page?.next_offset??0)">Next output</Button>
   <Button variant="outline" :disabled="busy" @click="tail">{{follow?'Following latest':'Follow latest'}}</Button>
   <span v-if="page">Bytes {{offset}}–{{page.next_offset}} / {{page.end_offset}}</span>
  </div>
  <p v-if="page?.dropped_bytes" class="text-sm text-muted-foreground">{{page.dropped_bytes}} earlier bytes were rotated out of retained history.</p>
  <pre class="max-h-96 overflow-auto whitespace-pre-wrap break-words rounded border p-3 text-sm">{{page?.text||(busy?'Loading output…':'(No output)')}}</pre>
 </section>
</template>

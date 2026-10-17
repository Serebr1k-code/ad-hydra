<script setup lang="ts">
import { computed,onMounted,onUnmounted,ref } from 'vue'
import type { Capture,Sample } from '@/generated/contracts'
import { api,ApiError,errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Alert,AlertDescription } from '@/components/ui/alert'
import { Select,SelectContent,SelectItem,SelectTrigger,SelectValue } from '@/components/ui/select'
import { bytePreview } from './traffic'
const props=withDefaults(defineProps<{flowId:string;live?:boolean;autoCapture?:boolean}>(),{live:true,autoCapture:false})
const capture=ref<Capture>(),sample=ref<Sample>(),busy=ref(false),error=ref(''),mode=ref('hex')
const preview=computed(()=>bytePreview(sample.value?.data??[]))
let timer:ReturnType<typeof setTimeout>|undefined,disposed=false
async function read(deadline:number) {
 if(!capture.value||disposed)return
 try {const value=await api<Sample>(`/traffic/samples/${encodeURIComponent(capture.value.sample_id)}`);if(!disposed){sample.value=value;busy.value=false}}
 catch(e){
  if(disposed)return
  if(e instanceof ApiError&&e.status===410&&Date.now()<deadline){timer=setTimeout(()=>void read(deadline),500);return}
  error.value=e instanceof ApiError&&e.status===410?'No sample arrived before capture expired. Capture another chunk on a live session.':errorMessage(e);busy.value=false
 }
}
async function request() {
 clearTimeout(timer);busy.value=true;error.value='';sample.value=undefined;capture.value=undefined
 try {
  const value=await api<Capture>(`/traffic/flows/${encodeURIComponent(props.flowId)}/capture`,'POST')
  if(disposed)return
  capture.value=value;void read(Date.now()+30000)
 }catch(e){if(!disposed){error.value=errorMessage(e);busy.value=false}}
}
onMounted(()=>{if(props.autoCapture&&props.live)void request()})
onUnmounted(()=>{disposed=true;clearTimeout(timer)})
</script>
<template>
 <section class="space-y-4">
  <p class="text-sm text-muted-foreground">Raw sessions open with a one-shot capture of the next chunk. Samples expire after 30 seconds; bytes before the capture request are not retained. A sample does not include direction metadata. Capture another chunk at any time.</p>
  <div class="flex flex-wrap gap-2"><Button variant="outline" :disabled="busy||!live" @click="request">{{busy?'Waiting for traffic…':'Capture next chunk'}}</Button><Select v-model="mode"><SelectTrigger aria-label="Byte display" class="w-36"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="hex">Hex dump</SelectItem><SelectItem value="utf8">UTF-8</SelectItem><SelectItem value="ascii">ASCII</SelectItem></SelectContent></Select></div>
  <p v-if="!live" class="text-sm text-muted-foreground">This session is closed. New byte samples require a live session.</p>
  <Alert v-if="error" variant="destructive"><AlertDescription>{{error}}</AlertDescription></Alert>
  <template v-if="sample"><p class="text-xs text-muted-foreground">{{sample.data.length}} captured bytes · expires {{sample.expires_at}}{{sample.truncated?' · captured chunk truncated':''}}</p><pre class="!max-h-none font-mono !text-[13px]">{{(mode==='ascii'?preview.ascii:mode==='utf8'?preview.utf8:preview.hex)||'(empty)'}}</pre></template>
 </section>
</template>

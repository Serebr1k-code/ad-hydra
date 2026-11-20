<script setup lang="ts">
import { ref } from 'vue'
import type { ImportResult } from '@/generated/contracts'
import { api,errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { Select,SelectContent,SelectItem,SelectTrigger,SelectValue } from '@/components/ui/select'
defineProps<{services:{id:string;name:string}[]}>()
const target=defineModel<string>('target',{required:true})
const emit=defineEmits<{saved:[]}>()
const text=ref(''),service=ref('none'),error=ref(''),notice=ref(''),busy=ref(false)
async function save(){
 busy.value=true;error.value='';notice.value=''
 try{
  if(new TextEncoder().encode(text.value).byteLength>65536)throw new Error('Paste at most 64 KiB of text.')
  const result=await api<ImportResult>('/flags/import','POST',{text:text.value,service_id:service.value==='none'?null:service.value,target_id:target.value||null})
  notice.value=`${result.matched} matches · ${result.added} new flags · ${result.queued} queued · ${result.duplicates} duplicates.`
  text.value='';emit('saved')
 }catch(cause){error.value=errorMessage(cause)}finally{busy.value=false}
}
</script>
<template>
 <section class="space-y-3 rounded border p-4">
  <h2 class="text-lg font-semibold">Paste captured flags</h2>
  <form class="space-y-3" @submit.prevent="save">
   <div><Label for="flag-paste">Text containing flags</Label><Textarea id="flag-paste" v-model="text" rows="5" required :disabled="busy" placeholder="AAAAAABBBBBBCCCCCCDDDDDDEEEEEE1=&#10;Paste output or a list of captured flags…" autocomplete="off" spellcheck="false" /></div>
   <div class="grid gap-3 sm:grid-cols-2"><div><Label>Service (optional)</Label><Select v-model="service" :disabled="busy"><SelectTrigger aria-label="Pasted flags service"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">Unspecified</SelectItem><SelectItem v-for="item in services" :key="item.id" :value="item.id">{{item.name}}</SelectItem></SelectContent></Select></div><div><Label for="flag-target">Authorized target ID (optional)</Label><Input id="flag-target" v-model="target" maxlength="128" :disabled="busy" /></div></div>
   <p class="text-sm text-muted-foreground">The saved extraction regex selects full matches verbatim. Flags are not trimmed, recased or decoded. Duplicates queue once; up to 256 matches per paste. Automatic submission sends queued flags when enabled; otherwise use Run saved submitter now.</p>
   <p v-if="error" role="alert" class="text-destructive">{{error}}</p><p v-if="notice" role="status">{{notice}}</p>
   <Button type="submit" :disabled="busy">{{busy?'Parsing…':'Parse and queue flags'}}</Button>
  </form>
 </section>
</template>

<script setup lang="ts">
import {computed,onMounted,ref,watch} from 'vue'
import {Checkbox} from '@/components/ui/checkbox'
import {Input} from '@/components/ui/input'
import {Label} from '@/components/ui/label'
import {Select,SelectContent,SelectItem,SelectTrigger,SelectValue} from '@/components/ui/select'
import {api,errorMessage} from '@/shared/api/client'
import type {ForwardingDraft,ForwardFraming} from './forwarding'
const props=defineProps<{disabled:boolean;transport:string;listenAddress:string}>()
const model=defineModel<ForwardingDraft|null>({required:true})
const teams=ref<{id:string;name:string;host:string;enabled:boolean}[]>([]),loading=ref(false),error=ref('')
const enabled=computed({get:()=>model.value!==null,set:(value:boolean)=>{model.value=value?{team_id:'',target_port:Number(props.listenAddress.split(':').at(-1))||9000,flag_pattern:'[A-Z0-9]{31}=',framing:props.transport==='udp'?{type:'datagram'}:{type:'http1'}}:null}})
const framing=computed({get:()=>model.value?.framing.type??'http1',set:(type:string)=>{if(model.value)model.value.framing=type==='delimited'?{type:'delimited',delimiter:'\n'}:type==='length_prefixed'?{type:'length_prefixed',offset:0,width:4,little_endian:false,length_includes_header:false}:{type:type as 'http1'|'datagram'}}})
function setFraming(value:Partial<ForwardFraming>){if(model.value)model.value.framing={...model.value.framing,...value} as ForwardFraming}
watch(()=>props.transport,transport=>{if(model.value)model.value.framing=transport==='udp'?{type:'datagram'}:{type:'http1'}})
onMounted(async()=>{loading.value=true;try{let after='';do{const page=await api<{items:typeof teams.value;next_cursor:string|null}>('/farm/teams'+(after?'?after='+encodeURIComponent(after):''));teams.value.push(...page.items);after=page.next_cursor??''}while(after&&teams.value.length<1024)}catch(cause){error.value=errorMessage(cause)}finally{loading.value=false}})
</script>
<template>
 <fieldset id="service-forwarding-settings" tabindex="-1" class="scroll-mt-4 space-y-3 rounded-md border p-3" :disabled="disabled">
  <div class="flex items-center gap-2"><Checkbox id="service-forwarding" v-model="enabled" :disabled="disabled"/><Label for="service-forwarding">{{transport==='tcp'?'TCP mirroring':'UDP forwarding'}} · flag substitution</Label></div>
  <template v-if="model">
   <p class="text-sm text-muted-foreground">Clients keep this listener. Requests go to the selected team with generated matching flags; known substitutes in replies are restored. Mappings persist across connections and restarts.</p>
   <div class="space-y-1"><Label for="forward-team">Target team</Label><Select v-model="model.team_id" :disabled="disabled||loading"><SelectTrigger id="forward-team"><SelectValue :placeholder="loading?'Loading teams…':'Select a team'"/></SelectTrigger><SelectContent><SelectItem v-for="team in teams.filter(t=>t.enabled)" :key="team.id" :value="team.id">{{team.name}} · {{team.host}}</SelectItem></SelectContent></Select><p v-if="!loading&&!teams.some(t=>t.enabled)" class="text-sm text-muted-foreground">Add an enabled team in Farm first.</p></div>
   <div class="space-y-1"><Label for="forward-port">Target service port</Label><Input id="forward-port" v-model.number="model.target_port" type="number" min="1" max="65535" required/></div>
   <div class="space-y-1"><Label for="forward-pattern">Flag format</Label><Input id="forward-pattern" v-model="model.flag_pattern" required class="font-mono"/><p class="text-xs text-muted-foreground">Fixed-width printable ASCII regex: literals, character classes and exact repeats. Example: [A-Z0-9]{31}=</p></div>
   <div v-if="transport==='tcp'" class="space-y-1"><Label for="forward-framing">TCP message framing</Label><Select v-model="framing" :disabled="disabled"><SelectTrigger id="forward-framing"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="http1">HTTP/1.1</SelectItem><SelectItem value="delimited">Delimited messages</SelectItem><SelectItem value="length_prefixed">Length-prefixed messages</SelectItem></SelectContent></Select></div>
   <div v-if="model.framing.type==='delimited'" class="space-y-1"><Label>Delimiter</Label><Select :model-value="model.framing.delimiter==='\r\n'?'crlf':'lf'" @update:model-value="setFraming({delimiter:$event==='crlf'?'\r\n':'\n'})"><SelectTrigger aria-label="Message delimiter"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="lf">Line feed (LF)</SelectItem><SelectItem value="crlf">CRLF</SelectItem></SelectContent></Select></div>
   <div v-if="model.framing.type==='length_prefixed'" class="grid grid-cols-2 gap-3">
    <div><Label for="forward-offset">Length field offset</Label><Input id="forward-offset" v-model.number="model.framing.offset" type="number" min="0" max="16"/></div>
    <div><Label for="forward-width">Length field bytes</Label><Select :model-value="String(model.framing.width)" @update:model-value="setFraming({width:Number($event)})"><SelectTrigger id="forward-width"><SelectValue/></SelectTrigger><SelectContent><SelectItem v-for="width in [1,2,4]" :key="width" :value="String(width)">{{width}}</SelectItem></SelectContent></Select></div>
    <div class="flex items-center gap-2"><Checkbox id="forward-le" v-model="model.framing.little_endian"/><Label for="forward-le">Little endian</Label></div><div class="flex items-center gap-2"><Checkbox id="forward-header" v-model="model.framing.length_includes_header"/><Label for="forward-header">Length includes header</Label></div>
   </div>
   <p class="text-xs text-muted-foreground">Messages are limited to 64 KiB, with a 5-second deadline for incomplete frames. TLS, compressed bodies, upgrades and signed payloads require a protocol-specific rewrite adapter. The target sees Hydra's source address. Full mapping storage rejects new flags; mappings are never automatically rotated.</p>
  </template>
  <p v-if="error" role="alert" class="text-sm text-destructive">{{error}}</p>
 </fieldset>
</template>

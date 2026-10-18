<script setup lang="ts">
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { serviceColor,type Flow } from './traffic'
import { resolveServiceColors } from '@/features/services/colors'
const props=defineProps<{rows:Flow[];showBlocked:boolean;services:{id:string;name:string;color?:string|null}[];service:string;selected:string;loading:boolean;more:boolean;filterSelection:string[];filterSelectionService:string}>()
const emit=defineEmits<{service:[value:string];showBlocked:[value:boolean];select:[flow:Flow];refresh:[];more:[];toggleFilter:[flow:Flow,checked:boolean]}>()
const search=ref('')
const colors=computed(()=>resolveServiceColors(props.services))
function color(id:string){return colors.value[id]??serviceColor(id)}
const visible=computed(()=>props.rows.filter(f=>`${f.client} ${f.destination} ${f.flow_id}`.toLowerCase().includes(search.value.toLowerCase())))
</script>
<template>
 <aside id="traffic-sessions" aria-label="TCP sessions" class="flex w-full max-h-80 min-h-0 flex-col border-b md:max-h-none md:border-b-0 md:border-r">
  <div class="space-y-3 border-b p-4">
   <div class="space-y-1"><Label for="traffic-service">Service</Label><Select :model-value="service" @update:model-value="emit('service',String($event))"><SelectTrigger id="traffic-service"><SelectValue placeholder="All services" /></SelectTrigger><SelectContent><SelectItem value="all">All services</SelectItem><SelectItem v-for="s in services" :key="s.id" :value="s.id"><span aria-hidden="true" class="mr-2 inline-block size-2 rounded-full" :style="{backgroundColor:color(s.id)}"></span>{{s.name}}</SelectItem></SelectContent></Select></div>
   <label class="flex items-center gap-2 text-sm"><Checkbox id="traffic-show-blocked" :model-value="showBlocked" @update:model-value="emit('showBlocked',$event===true)" />Show blocked</label>
   <Input v-model="search" aria-label="Find session" placeholder="Find address or session…" />
   <div class="flex items-center justify-between"><span class="text-xs text-muted-foreground">{{rows.length}} sessions loaded</span><Button size="sm" variant="ghost" :disabled="loading" @click="emit('refresh')">{{loading?'Loading…':'Refresh'}}</Button></div>
  </div>
  <div class="min-h-0 flex-1 overflow-y-auto p-2" aria-label="Sessions">
   <div v-for="flow in visible" :key="flow.flow_id" data-stream-card class="relative mb-2 rounded-md border border-l-4" :style="{borderLeftColor:color(flow.service_id)}" :class="selected===flow.flow_id?'bg-accent':flow.has_flag?'bg-amber-500/10 ring-1 ring-inset ring-amber-500/60':''">
   <Checkbox :model-value="filterSelection.includes(flow.flow_id)" :disabled="Boolean(filterSelectionService && filterSelectionService!==flow.service_id) || (filterSelection.length>=50 && !filterSelection.includes(flow.flow_id))" :aria-label="'Select stream for filter ' + flow.flow_id" class="absolute right-3 top-3 z-10" @update:model-value="value => emit('toggleFilter',flow,value===true)" />
   <Button :data-flow-id="flow.flow_id" :data-service-id="flow.service_id" :data-has-flag="flow.has_flag" :data-blocked="flow.blocked"   :aria-pressed="selected===flow.flow_id" :variant="selected===flow.flow_id?'secondary':'ghost'" class="h-auto w-full min-w-0 justify-start whitespace-normal py-3 pl-3 pr-10 text-left font-normal" @click="emit('select',flow)">
    <span class="block w-full min-w-0 space-y-1"><span class="flex items-center justify-between gap-2"><span class="truncate font-mono text-xs">{{flow.client}}</span><span v-if="flow.has_flag" class="shrink-0 font-medium text-amber-500">Flag</span><span v-if="flow.blocked" class="shrink-0 font-medium text-destructive">Blocked</span><span class="text-[10px] text-muted-foreground">{{flow.state}}</span></span><span class="block truncate font-mono text-xs text-muted-foreground">→ {{flow.destination}}</span><span class="flex justify-between gap-2 text-[10px] text-muted-foreground"><span class="truncate">{{flow.service_id}}</span><span>{{flow.started.slice(11,19)}}</span></span></span>
   </Button>
   </div>
   <p v-if="!loading&&!visible.length" class="p-4 text-sm text-muted-foreground">{{rows.length?'No sessions match your search.':'No sessions for this service yet.'}}</p>
   <Button v-if="more" variant="outline" class="mt-2 w-full" :disabled="loading||rows.length>=500" @click="emit('more')">More sessions</Button>
   <p v-if="rows.length>=500" class="p-2 text-xs text-muted-foreground">500-session limit. Narrow the time range to see other sessions.</p>
  </div>
 </aside>
</template>

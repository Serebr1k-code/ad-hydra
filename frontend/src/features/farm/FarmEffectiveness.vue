<script setup lang="ts">
import {computed,onMounted,onUnmounted,ref,watch} from 'vue'
import type {FarmService} from '@/generated/contracts'
import {api,errorMessage} from '@/shared/api/client'
import {date} from '@/shared/format'
import {Button} from '@/components/ui/button'
import {Label} from '@/components/ui/label'
import {Select,SelectContent,SelectItem,SelectTrigger,SelectValue} from '@/components/ui/select'
import {Table,TableBody,TableCell,TableHead,TableHeader,TableRow} from '@/components/ui/table'
import type {EffectivenessMatrix,EffectivenessRow,EffectivenessTeam} from './effectivenessTypes'
import {matrixCellKey,targetAttemptDuration} from './effectivenessTypes'
const props=defineProps<{services:FarmService[]}>()
const emit=defineEmits<{logs:[id:string]}>()
const service=ref('all'),window=ref('24h'),rowOffset=ref(0),teamOffset=ref(0)
const matrix=ref<EffectivenessMatrix|null>(null),loading=ref(false),error=ref(''),now=ref(Date.now())
let mounted=true,request=0,timer:ReturnType<typeof setInterval>|undefined
const cells=computed(()=>new Map((matrix.value?.cells??[]).map(cell=>[matrixCellKey(cell.exploit_id,cell.service_id,cell.team_key),cell])))
function cell(row:EffectivenessRow,team:EffectivenessTeam){return cells.value.get(matrixCellKey(row.exploit_id,row.service_id,team.key))}
async function load(){
 const serial=++request;loading.value=true;error.value=''
 const query=new URLSearchParams({window:window.value,row_offset:String(rowOffset.value),team_offset:String(teamOffset.value)})
 if(service.value!=='all')query.set('service_id',service.value==='legacy'?'':service.value.slice(8))
 try{const result=await api<EffectivenessMatrix>(`/farm/effectiveness?${query}`);if(mounted&&serial===request)matrix.value=result}
 catch(e){if(mounted&&serial===request)error.value=errorMessage(e)}
 finally{if(mounted&&serial===request)loading.value=false}
}
function reset(){rowOffset.value=0;teamOffset.value=0;matrix.value=null;void load()}
function page(axis:'rows'|'teams',offset:number){if(axis==='rows')rowOffset.value=offset;else teamOffset.value=offset;matrix.value=null;void load()}
watch([service,window],reset)
onMounted(()=>{void load();timer=setInterval(()=>{now.value=Date.now()},1000)})
onUnmounted(()=>{mounted=false;request++;if(timer)clearInterval(timer)})
</script>
<template>
 <section class="space-y-3" data-farm-effectiveness>
  <div class="flex flex-wrap items-end gap-3">
   <div class="mr-auto"><h2 class="text-lg font-semibold">Exploit effectiveness</h2><p class="text-sm text-muted-foreground">Exploit × team, from retained runs and actual flag submissions.</p></div>
   <div class="space-y-1"><Label for="matrix-service">Service</Label><Select v-model="service"><SelectTrigger id="matrix-service" class="w-44"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="all">All services</SelectItem><SelectItem value="legacy">Legacy targets</SelectItem><SelectItem v-for="item in props.services" :key="item.id" :value="`service:${item.id}`">{{item.name}}</SelectItem></SelectContent></Select></div>
   <div class="space-y-1"><Label for="matrix-window">Run window</Label><Select v-model="window"><SelectTrigger id="matrix-window" class="w-40"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="1h">Last hour</SelectItem><SelectItem value="24h">Last 24 hours</SelectItem><SelectItem value="7d">Last 7 days</SelectItem><SelectItem value="retained">Retained history</SelectItem></SelectContent></Select></div>
   <Button variant="outline" :disabled="loading" @click="load">Refresh matrix</Button>
  </div>
  <p v-if="error" role="alert" class="text-destructive">{{error}}</p>
  <p v-if="loading" role="status">Loading effectiveness matrix…</p>
  <p v-if="matrix&&(!matrix.rows.length||!matrix.teams.length)" class="text-muted-foreground">No exploits or teams match this view. Add catalogs or choose another window.</p>
  <div v-else-if="matrix" class="max-w-full overflow-x-auto rounded border">
   <Table aria-label="Exploit effectiveness by team">
    <TableHeader><TableRow><TableHead class="min-w-44">Exploit / service</TableHead><TableHead v-for="team in matrix.teams" :key="team.key" class="min-w-60"><span>{{team.name}}</span><span v-if="team.host" class="block font-mono text-xs">{{team.host}}</span><span v-if="team.key.startsWith('target:')" class="block text-xs">Legacy target</span></TableHead></TableRow></TableHeader>
    <TableBody><TableRow v-for="row in matrix.rows" :key="JSON.stringify([row.exploit_id,row.service_id])"><TableCell class="align-top"><div class="font-medium">{{row.name}}</div><div class="text-xs text-muted-foreground">{{row.service_name}}</div></TableCell>
     <TableCell v-for="team in matrix.teams" :key="team.key" class="align-top">
      <template v-if="cell(row,team)?.latest">
       <div class="flex items-baseline justify-between gap-2"><span class="font-medium">{{cell(row,team)?.latest?.process_status??'Target outcome unknown'}}</span><Button variant="link" class="h-auto p-0" :aria-label="`Logs for ${row.name} against ${team.name}`" @click="emit('logs',cell(row,team)!.latest!.id)">Logs</Button></div>
       <div class="text-xs text-muted-foreground">{{cell(row,team)?.runs}} runs · {{cell(row,team)?.target_attempts}} recorded target attempts</div>
       <div class="mt-1 text-sm">{{cell(row,team)?.new_flags}} new / {{cell(row,team)?.duplicate_flags}} duplicate<span v-if="cell(row,team)?.flag_counts_complete===false" title="Historical flag counts are partial"> *</span></div>
       <div class="text-sm">{{cell(row,team)?.accepted_flags}} accepted</div>
       <div class="mt-1 text-xs text-muted-foreground">Latest run: {{cell(row,team)?.latest?.status}} · {{cell(row,team)?.latest?.mode}}</div>
       <div class="text-xs text-muted-foreground">{{targetAttemptDuration(cell(row,team)!.latest!,now)}}</div>
       <div class="text-xs text-muted-foreground">{{date(cell(row,team)!.latest!.created_at)}}</div>
       <div v-if="cell(row,team)?.last_accepted_capture_at" class="text-xs text-muted-foreground">Last accepted capture: {{date(cell(row,team)!.last_accepted_capture_at!)}}</div>
      </template><span v-else class="text-sm text-muted-foreground">No runs</span>
     </TableCell>
    </TableRow></TableBody>
   </Table>
  </div>
  <div v-if="matrix" class="flex flex-wrap gap-2">
   <Button v-if="rowOffset" variant="outline" :disabled="loading" @click="page('rows',Math.max(0,rowOffset-20))">Previous exploits</Button><Button v-if="matrix.next_row_offset!=null" variant="outline" :disabled="loading" @click="page('rows',matrix.next_row_offset)">Next exploits</Button>
   <Button v-if="teamOffset" variant="outline" :disabled="loading" @click="page('teams',Math.max(0,teamOffset-10))">Previous teams</Button><Button v-if="matrix.next_team_offset!=null" variant="outline" :disabled="loading" @click="page('teams',matrix.next_team_offset)">Next teams</Button>
   <span class="self-center text-xs text-muted-foreground">Up to 20 exploits × 10 teams per page. Updated {{date(matrix.generated_at)}}.</span>
  </div>
  <p class="text-xs text-muted-foreground">Windows filter run creation time. New / duplicate counts are distinct values per run and target. Accepted flags count only their original capture; duplicate sightings receive no acceptance credit. Last accepted capture is the capture time, not checker response time. Batch processes and older runs have no individual target outcome. Target attempt durations include setup; * marks partial historical flag counts. Names use current catalogs, falling back to saved target names after deletion.</p>
 </section>
</template>

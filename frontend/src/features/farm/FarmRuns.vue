<script setup lang="ts">
import {onMounted,onUnmounted,ref} from 'vue'
import type { FarmRun } from './types'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { date } from '@/shared/format'
import {runDuration} from './runTime'
defineProps<{ runs: FarmRun[]; busy: boolean }>()
const emit = defineEmits<{ refresh: []; logs: [run: FarmRun]; cancel: [run: FarmRun]; retry: [run: FarmRun] }>()
const now=ref(Date.now()/1000)
let timer:ReturnType<typeof setInterval>|undefined
onMounted(()=>{timer=setInterval(()=>{now.value=Date.now()/1000},1000)})
onUnmounted(()=>{if(timer)clearInterval(timer)})
const columns = [{key:'time',label:'Run time'},{key:'teams',label:'Team'},{key:'exploit_name',label:'Exploit'},{key:'service_name',label:'Service'},{key:'flags',label:'New / duplicate'},{key:'status',label:'State'},{key:'actions',label:'Actions'}]
</script>
<template>
  <section class="space-y-3" data-farm-runs>
    <div class="flex items-center justify-between gap-2"><h2 class="text-lg font-semibold">Runs</h2><Button variant="outline" :disabled="busy" @click="emit('refresh')">Refresh runs</Button></div>
    <DataTable :columns="columns" :rows="runs">
      <template #cell-time="{row}"><div class="whitespace-nowrap">{{date(row.started_at??row.created_at)}}</div><div class="text-sm text-muted-foreground">{{runDuration(row,now)}}</div></template>
      <template #cell-teams="{row}"><ul v-if="row.teams?.length" class="space-y-1"><li v-for="team in row.teams" :key="team.id"><span>{{team.name}}</span><span v-if="team.host" class="block whitespace-nowrap font-mono text-xs text-muted-foreground">{{team.host}}</span></li></ul><span v-else>{{row.team_ids?.join(', ')||row.target_ids?.join(', ')||'Unspecified'}}</span></template>
      <template #cell-exploit_name="{row}">{{row.exploit_name??row.exploit_id}}</template>
      <template #cell-service_name="{row}">{{row.service_name??row.service_id??'Legacy targets'}}</template>
      <template #cell-flags="{row}">{{row.new_flags??0}} / {{row.existing_flags??0}}<span v-if="row.flag_counts_complete===false" title="Historical counts are partial">*</span></template>
      <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2">
        <Button variant="outline" :disabled="busy" @click="emit('logs', row)">Logs</Button>
        <Button v-if="['queued', 'running'].includes(row.status)" variant="outline" :disabled="busy" @click="emit('cancel', row)">Cancel</Button>
        <Button v-else variant="outline" :disabled="busy" @click="emit('retry', row)">Retry</Button>
      </div></template>
    </DataTable>
  </section>
</template>

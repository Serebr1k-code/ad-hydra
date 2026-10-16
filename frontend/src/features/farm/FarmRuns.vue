<script setup lang="ts">
import type { FarmRun } from './types'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { date } from '@/shared/format'
defineProps<{ runs: FarmRun[]; busy: boolean }>()
const emit = defineEmits<{ refresh: []; logs: [run: FarmRun]; cancel: [run: FarmRun]; retry: [run: FarmRun] }>()
const columns = [{ key: 'id', label: 'Run' }, { key: 'exploit_id', label: 'Exploit' }, { key: 'status', label: 'State' }, { key: 'created_at', label: 'Created' }, { key: 'actions', label: 'Actions' }]
</script>
<template>
  <section class="space-y-3">
    <div class="flex items-center justify-between gap-2"><h2 class="text-lg font-semibold">Runs</h2><Button variant="outline" :disabled="busy" @click="emit('refresh')">Refresh runs</Button></div>
    <DataTable :columns="columns" :rows="runs">
      <template #cell-created_at="{ row }">{{ date(row.created_at) }}</template>
      <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2">
        <Button variant="outline" :disabled="busy" @click="emit('logs', row)">Logs</Button>
        <Button v-if="['queued', 'running'].includes(row.status)" variant="outline" :disabled="busy" @click="emit('cancel', row)">Cancel</Button>
        <Button v-else variant="outline" :disabled="busy" @click="emit('retry', row)">Retry</Button>
      </div></template>
    </DataTable>
  </section>
</template>

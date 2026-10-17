<script setup lang="ts">
import type { FlagView } from '@/generated/contracts'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { date } from '@/shared/format'
defineProps<{ flags: FlagView[]; serviceNames: Record<string,string>; busy: boolean }>()
const emit = defineEmits<{ submit: [flag: FlagView]; triage: [flag: FlagView, resolution: string] }>()
const columns = [{ key: 'service_id', label: 'Service' }, { key: 'state', label: 'Submission' }, { key: 'category', label: 'Category' }, { key: 'digest', label: 'Digest' }, { key: 'target_id', label: 'Target' }, { key: 'run_id', label: 'Run' }, { key: 'initiated_by', label: 'Initiated by' }, { key: 'created_at', label: 'Created' }, { key: 'actions', label: 'Actions' }]
</script>
<template>
  <section class="space-y-3"><h2 class="text-lg font-semibold">Flags</h2>
    <DataTable :columns="columns" :rows="flags">
      <template #cell-service_id="{ row }">{{serviceNames[row.service_id??'']??row.service_id??'Unspecified'}}</template>
      <template #cell-created_at="{ row }">{{ date(row.created_at) }}</template>
      <template #cell-actions="{ row }">
        <Button v-if="row.category === 'captured' && row.triage !== 'own_exposed'" variant="outline" :disabled="busy" @click="emit('submit', row)">Submit</Button>
        <div v-else-if="row.category === 'unknown' && !row.triage" class="flex flex-wrap gap-2"><Button v-for="resolution in ['dismissed', 'captured']" :key="resolution" variant="outline" :disabled="busy" @click="emit('triage', row, resolution)">{{ resolution }}</Button></div>
        <span v-else>{{ row.triage || 'Defensive finding' }}</span>
      </template>
    </DataTable>
  </section>
</template>

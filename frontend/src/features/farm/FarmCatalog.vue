<script setup lang="ts">
import { ref, watch } from 'vue'
import type { FarmService, FarmTeam } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { serviceDto, teamDto } from './farmDto'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
const props = defineProps<{ kind: 'services' | 'teams'; items: (FarmService | FarmTeam)[] }>()
const emit = defineEmits<{ saved: [] }>()
const selected = ref<FarmService | FarmTeam | null>(null)
const name = ref(''), host = ref(''), port = ref<string | number>(9000)
const busy = ref(false), error = ref('')
const columns = [{ key: 'name', label: 'Name' }, { key: 'destination', label: 'Destination' }, { key: 'enabled', label: 'Enabled' }, { key: 'actions', label: 'Actions' }]
watch(selected, value => { name.value = value?.name ?? ''; host.value = value && 'host' in value ? value.host : ''; port.value = value && 'port' in value ? value.port : 9000; error.value = '' })
async function perform(work: () => Promise<unknown>) {
  if (busy.value) return
  busy.value = true; error.value = ''
  try { await work(); selected.value = null; name.value = ''; host.value = ''; port.value = 9000; emit('saved') }
  catch (e) { error.value = errorMessage(e) }
  finally { busy.value = false }
}
function save() {
  const common = { id: selected.value?.id ?? crypto.randomUUID(), name: name.value.trim(), enabled: selected.value?.enabled ?? true, revision: selected.value?.revision ?? 0 }
  const value = props.kind === 'services' ? serviceDto({ ...common, port: Number(port.value), transport: selected.value && 'transport' in selected.value ? selected.value.transport : 'tcp' }) : teamDto({ ...common, host: host.value.trim() })
  void perform(() => api(`/farm/${props.kind}`, 'PUT', value))
}
function toggle(row: FarmService | FarmTeam) { void perform(() => api(`/farm/${props.kind}`, 'PUT', 'port' in row ? serviceDto({ ...row, enabled: !row.enabled }) : teamDto({ ...row, enabled: !row.enabled }))) }
function remove(row: FarmService | FarmTeam) { void perform(() => api(`/farm/${props.kind}/${encodeURIComponent(row.id)}?revision=${row.revision}`, 'DELETE')) }
</script>
<template>
  <section class="space-y-3 rounded border p-4">
    <h2 class="text-lg font-semibold">{{ kind === 'services' ? 'Services' : 'Teams' }}</h2>
    <p class="text-sm text-muted-foreground">{{ kind === 'services' ? 'Define the destination port for each competition service.' : 'Define the authorized numeric IP address for each team.' }}</p>
    <DataTable :columns="columns" :rows="items">
      <template #cell-destination="{ row }">{{ 'host' in row ? row.host : `${row.port}/${row.transport}` }}</template>
      <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2"><Button variant="outline" :disabled="busy" @click="selected = row">Edit</Button><Button variant="outline" :disabled="busy" @click="toggle(row)">{{ row.enabled ? 'Disable' : 'Enable' }}</Button><Button variant="outline" :disabled="busy" @click="remove(row)">Delete</Button></div></template>
    </DataTable>
    <form class="flex flex-wrap items-end gap-3" @submit.prevent="save">
      <div><Label :for="`${kind}-name`">{{ selected ? 'Edit' : 'Add' }} {{ kind === 'services' ? 'service' : 'team' }} name</Label><Input :id="`${kind}-name`" v-model="name" required maxlength="128" :disabled="busy" /></div>
      <div v-if="kind === 'services'"><Label for="farm-service-port">Port</Label><Input id="farm-service-port" v-model="port" type="number" min="1" max="65535" required :disabled="busy" /></div>
      <div v-else><Label for="farm-team-host">Numeric IP address</Label><Input id="farm-team-host" v-model="host" required :disabled="busy" placeholder="10.60.1.2" /></div>
      <Button type="submit" :disabled="busy">{{ busy ? 'Saving…' : selected ? 'Save changes' : kind === 'services' ? 'Add service' : 'Add team' }}</Button>
      <Button v-if="selected" type="button" variant="outline" :disabled="busy" @click="selected = null">Cancel edit</Button>
    </form>
    <p v-if="error" role="alert" class="text-destructive">{{ error }}</p>
  </section>
</template>

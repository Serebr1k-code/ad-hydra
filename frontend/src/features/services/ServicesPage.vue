<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { ListResponse, ServiceInput } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { openEditor } from '@/shared/editor'
import { Button } from '@/components/ui/button'
import DataTable from '@/components/DataTable.vue'

type Service = Omit<ServiceInput, 'expected_revision'> & { id: string; applied_enabled: boolean | null }
type Services = Omit<ListResponse, 'items'> & { items: Service[] }
const data = ref<Services | null>(null)
const busy = ref(false)
const error = ref('')
const columns = [{ key: 'transport', label: 'Transport' }, { key: 'listen_address', label: 'Listener' }, { key: 'upstream_address', label: 'Upstream' }, { key: 'protocol_id', label: 'Profile' }, { key: 'enabled', label: 'Desired' }, { key: 'applied_enabled', label: 'Applied' }, { key: 'actions', label: 'Actions' }]
async function load() {
  busy.value = true
  error.value = ''
  try { data.value = await api<Services>('/services') }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
function edit(service?: Service) {
  const initial = service ? { transport: service.transport, listen_address: service.listen_address, upstream_address: service.upstream_address, protocol_id: service.protocol_id, enabled: service.enabled } : { transport: 'tcp', listen_address: '127.0.0.1:9000', upstream_address: '127.0.0.1:9001', protocol_id: 'raw', enabled: true }
  openEditor(service ? 'Edit service' : 'Create service', initial, async value => {
    const current = await api<ListResponse>('/services')
    await api(`/services${service ? '/' + encodeURIComponent(service.id) : ''}`, service ? 'PATCH' : 'POST', { ...value, expected_revision: current.revision })
    await load()
  })
}
async function remove(service: Service) {
  if (!window.confirm('Delete this service?')) return
  busy.value = true
  error.value = ''
  try {
    const current = await api<ListResponse>('/services')
    await api(`/services/${encodeURIComponent(service.id)}?expected_revision=${current.revision}`, 'DELETE')
    await load()
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
onMounted(load)
</script>

<template>
  <section class="space-y-4">
    <h1 class="text-2xl font-semibold">Services</h1>
    <div class="flex flex-wrap items-center gap-3">
      <p v-if="data" class="text-sm text-muted-foreground">Revision {{ data.revision }} · {{ data.apply_status }} · applied {{ data.applied_revision }}</p>
      <Button :disabled="busy" @click="edit()">Add service</Button>
      <Button variant="outline" :disabled="busy" @click="load">Refresh</Button>
    </div>
    <p v-if="error || data?.apply_error" role="alert" class="text-sm text-destructive">{{ error || data?.apply_error }}</p>
    <p v-if="busy" role="status">Loading services…</p>
    <p v-else-if="data && !data.items.length" class="text-muted-foreground">No services configured. Add a listener and its defended upstream.</p>
    <DataTable v-if="data?.items.length" :columns="columns" :rows="data.items">
      <template #cell-enabled="{ row }">{{ row.enabled ? 'On' : 'Off' }}</template>
      <template #cell-applied_enabled="{ row }">{{ row.applied_enabled === null ? 'Pending' : row.applied_enabled ? 'On' : 'Off' }}</template>
      <template #cell-actions="{ row }"><div class="flex gap-2"><Button variant="outline" size="sm" :disabled="busy" @click="edit(row)">Edit</Button><Button variant="destructive" size="sm" :disabled="busy" @click="remove(row)">Delete</Button></div></template>
    </DataTable>
  </section>
</template>

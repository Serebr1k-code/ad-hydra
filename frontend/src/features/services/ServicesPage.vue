<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { ListResponse, ServiceInput } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import DataTable from '@/components/DataTable.vue'
import ServiceForm from './ServiceForm.vue'
import {decoderLabel,type DecoderCatalog} from './decoders'
const catalog=ref<DecoderCatalog>({items:[],activations:[]})

type Service = Omit<ServiceInput, 'expected_revision'> & { id: string; applied_enabled: boolean | null }
type Services = Omit<ListResponse, 'items'> & { items: Service[] }
const data = ref<Services | null>(null)
const busy = ref(false)
const error = ref('')
const formOpen = ref(false)
const formBusy = ref(false)
const editing = ref<Service>()
const formRevision = ref(0)
const formGeneration = ref(0)
const columns = [{ key: 'transport', label: 'Transport' }, { key: 'listen_address', label: 'Public listener' }, { key: 'upstream_address', label: 'Application backend' }, { key: 'protocol_id', label: 'Decoder' }, { key: 'enabled', label: 'Desired' }, { key: 'applied_enabled', label: 'Applied' }, { key: 'actions', label: 'Actions' }]
async function load() {
  busy.value = true
  error.value = ''
  try { const [listeners,decoders]=await Promise.all([api<Services>('/services'),api<DecoderCatalog>('/protocols')]);data.value=listeners;catalog.value=decoders }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
function edit(service?: Service) {
  if (!data.value) return
  editing.value = service
  formRevision.value = data.value.revision
  formGeneration.value++
  formOpen.value = true
}
async function saved() {
  formOpen.value = false
  await load()
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
    <p class="text-sm text-muted-foreground">Keep each service's original public port on Hydra and move its application to a separate private backend port. The application sees Hydra's source address.</p>
    <div class="flex flex-wrap items-center gap-3">
      <p v-if="data" class="text-sm text-muted-foreground">Revision {{ data.revision }} · {{ data.apply_status }} · applied {{ data.applied_revision }}</p>
      <Button :disabled="busy || !data" @click="edit()">Add service</Button>
      <Button variant="outline" :disabled="busy" @click="load">Refresh</Button>
    </div>
    <p v-if="error || data?.apply_error" role="alert" class="text-sm text-destructive">{{ error || data?.apply_error }}</p>
    <p v-if="busy" role="status">Loading services…</p>
    <p v-else-if="data && !data.items.length" class="text-muted-foreground">No services configured. Add a listener and its defended upstream.</p>
    <DataTable v-if="data?.items.length" :columns="columns" :rows="data.items">
      <template #cell-protocol_id="{row}">{{decoderLabel(row,catalog)}}</template>
      <template #cell-enabled="{ row }">{{ row.enabled ? 'On' : 'Off' }}</template>
      <template #cell-applied_enabled="{ row }">{{ row.applied_enabled === null ? 'Pending' : row.applied_enabled ? 'On' : 'Off' }}</template>
      <template #cell-actions="{ row }"><div class="flex gap-2"><Button variant="outline" size="sm" :disabled="busy" @click="edit(row)">Edit</Button><Button variant="destructive" size="sm" :disabled="busy" @click="remove(row)">Delete</Button></div></template>
    </DataTable>
    <Dialog :open="formOpen" @update:open="value => { if (!formBusy) formOpen = value }">
      <DialogContent class="max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{{ editing ? 'Edit service' : 'Create service' }}</DialogTitle>
          <DialogDescription>Clients → public listener → application backend. Saving an enabled service applies its listener configuration to Proxy.</DialogDescription>
        </DialogHeader>
        <ServiceForm v-if="formOpen" :key="formGeneration" :service="editing" :revision="formRevision" @busy="value => formBusy = value" @saved="saved" />
      </DialogContent>
    </Dialog>
  </section>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import type { ListResponse, ServiceInput } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import DataTable from '@/components/DataTable.vue'
import ServiceForm from './ServiceForm.vue'
import ServiceMetrics from './ServiceMetrics.vue'
import {forwardingDestination} from './forwarding'
import type { ProtectionService } from './protection'
import {decoderLabel,type DecoderCatalog} from './decoders'
import ServiceColorDialog from './ServiceColorDialog.vue'
import { resolveServiceColors } from './colors'
const catalog=ref<DecoderCatalog>({items:[],activations:[]})

type Service = Omit<ServiceInput, 'expected_revision'> & ProtectionService & { id: string; name?:string; applied_enabled: boolean | null; color?: string | null }
type Services = Omit<ListResponse, 'items'> & { items: Service[] }
const data = ref<Services | null>(null)
const busy = ref(false)
const error = ref('')
const formOpen = ref(false)
const formBusy = ref(false)
const editing = ref<Service>()
const coloring = ref<Service>()
const colors = computed(() => resolveServiceColors(data.value?.items ?? []))
const formSection=ref<'general'|'forwarding'>('general')
const formRevision = ref(0)
const formGeneration = ref(0)
const columns = [{key:'name',label:'Name'},{ key: 'color', label: 'Color' }, { key: 'transport', label: 'Transport' }, { key: 'listen_address', label: 'Listener' }, { key: 'destination', label: 'Destination' }, { key: 'protocol_id', label: 'Decoder' }, { key: 'actions', label: 'Buttons' }]
async function load() {
  busy.value = true
  error.value = ''
  try { const [listeners,decoders]=await Promise.all([api<Services>('/services'),api<DecoderCatalog>('/protocols')]);data.value=listeners;catalog.value=decoders }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
function edit(service?: Service,section:'general'|'forwarding'='general') {
  formSection.value=section
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
    <ServiceColorDialog :service="coloring" :color="coloring ? colors[coloring.id] ?? '#4dabf7' : '#4dabf7'" @close="coloring=undefined" @saved="load" />
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
      <template #cell-name="{row}"><span class="font-medium">{{row.name||row.id}}</span><span class="mt-1 block text-xs" :class="row.applied_enabled===null?'text-amber-400':row.enabled?'text-green-400':'text-muted-foreground'">{{row.applied_enabled===null?'Pending application':row.enabled?'Enabled':'Disabled'}}</span></template>
      <template #cell-color="{row}"><Button variant="ghost" size="sm" class="size-8 p-2" :aria-label="'Set color for ' + (row.name||row.id)" @click="coloring=row"><span class="size-4 shrink-0 rounded-sm border border-white/10" :style="{backgroundColor:colors[row.id]}" aria-hidden="true"></span></Button></template>
      <template #cell-transport="{row}"><span class="uppercase">{{row.transport}}</span></template>
      <template #cell-listen_address="{row}"><span class="whitespace-nowrap font-mono text-xs">{{row.listen_address}}</span></template>
      <template #cell-protocol_id="{row}">{{decoderLabel(row,catalog)}}</template>
      <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2"><Button variant="outline" size="sm" :disabled="busy" @click="edit(row)">Edit</Button><Button variant="outline" size="sm" :disabled="busy" @click="edit(row,'forwarding')">{{row.transport==='tcp'?'TCP mirroring':'Forwarding'}}</Button><Button variant="destructive" size="sm" :disabled="busy" @click="remove(row)">Delete</Button></div></template>
      <template #cell-destination="{row}"><span class="whitespace-nowrap font-mono text-xs">{{forwardingDestination(row)}}</span><span v-if="row.forwarding" class="block text-xs text-muted-foreground">{{row.transport==='tcp'?'TCP mirroring':'Forwarding'}} · flag substitution</span></template>
    </DataTable>
    <ServiceMetrics :services="data?.items ?? []" :colors="colors" />
    <Dialog :open="formOpen" @update:open="value => { if (!formBusy) formOpen = value }">
      <DialogContent class="max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{{ editing ? 'Edit service' : 'Create service' }}</DialogTitle>
          <DialogDescription>Clients → public listener → application backend. Saving an enabled service applies its listener configuration to Proxy.</DialogDescription>
        </DialogHeader>
        <ServiceForm v-if="formOpen" :key="formGeneration" :service="editing" :revision="formRevision" :initial-section="formSection" @busy="value => formBusy = value" @saved="saved" />
      </DialogContent>
    </Dialog>
  </section>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { ListResponse } from '@/generated/contracts'
import { api, uploadProtocol, activateProtocol, errorMessage } from '@/shared/api/client'
import { openEditor } from '@/shared/editor'
import { date } from '@/shared/format'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import DataTable from '@/components/DataTable.vue'

type Protocol = { id: string; version: string; error: string | null; state: string; created_by: string; created_at: string; profile: unknown; metadata: unknown }
type Activation = { service_id: string; id: string; version: string; applied: boolean; error: string | null }
type Protocols = { items: Protocol[]; activations: Activation[]; native_schemas?: unknown }
type Service = { id: string; listen_address: string }
type Services = Omit<ListResponse, 'items'> & { items: Service[] }
type ApplyResult = { applied?: boolean; error?: string | null }
const data = ref<Protocols | null>(null)
const services = ref<Service[]>([])
const targets = ref<Record<string, string>>({})
const protocolId = ref('')
const version = ref('')
const file = ref<File | null>(null)
const busy = ref(false)
const error = ref('')
const detailOpen = ref(false)
const detailTitle = ref('')
const detailText = ref('')
const columns = [{ key: 'id', label: 'ID' }, { key: 'version', label: 'Version' }, { key: 'state', label: 'State' }, { key: 'created_by', label: 'Author' }, { key: 'created_at', label: 'Uploaded' }, { key: 'actions', label: 'Actions' }]
const activationColumns = [{ key: 'service_id', label: 'Service' }, { key: 'id', label: 'Protocol' }, { key: 'version', label: 'Version' }, { key: 'applied', label: 'Applied' }, { key: 'actions', label: 'Parser' }]
function key(protocol: Protocol) { return JSON.stringify([protocol.id, protocol.version]) }
function path(protocol: Protocol) { return `/protocols/${encodeURIComponent(protocol.id)}/${encodeURIComponent(protocol.version)}` }
function showDetail(title: string, value: unknown) {
  detailTitle.value = title
  detailText.value = JSON.stringify(value, null, 2) ?? 'No details available'
  detailOpen.value = true
}
async function load() {
  busy.value = true
  error.value = ''
  try {
    const [protocols, listeners] = await Promise.all([api<Protocols>('/protocols'), api<Services>('/services')])
    data.value = protocols
    services.value = listeners.items
    for (const protocol of protocols.items) {
      const selected = targets.value[key(protocol)]
      if (!listeners.items.some(service => service.id === selected)) targets.value[key(protocol)] = listeners.items[0]?.id ?? ''
    }
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
function selectFile(event: Event) {
  const input = event.target
  file.value = input instanceof HTMLInputElement ? input.files?.[0] ?? null : null
}
async function mutate(action: () => Promise<unknown>) {
  busy.value = true
  error.value = ''
  try { await action(); await load() }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
async function upload() {
  await mutate(async () => {
    if (!file.value) throw new Error('Select a WASM module')
    const result = await uploadProtocol(protocolId.value, version.value, file.value)
    if (result.error) throw new Error(result.error)
  })
}
function framing() {
  openEditor('Immutable framing profile', { id: 'custom', version: '1', max_frame: 8192, framing: { kind: 'delimiter', delimiter: [10] } }, async input => {
    const { id, version: profileVersion, ...profile } = input
    if (typeof id !== 'string' || !id || typeof profileVersion !== 'string' || !profileVersion) throw new Error('Protocol ID and version are required')
    const result = await api<ApplyResult>(`/protocol-profiles/${encodeURIComponent(id)}/${encodeURIComponent(profileVersion)}`, 'PUT', profile)
    if (result.error) throw new Error(result.error)
    await load()
  })
}
async function activate(protocol: Protocol) {
  await mutate(async () => {
    const target = targets.value[key(protocol)]
    if (!target) throw new Error('Select a service')
    const result = await activateProtocol(protocol.id, protocol.version, target)
    if (!result.applied) throw new Error(result.error || 'Activation pending; refresh to check applied state')
  })
}
async function deactivate(activation: Activation) {
  await mutate(async () => {
    const result = await api<ApplyResult>(`/protocols/activations/${encodeURIComponent(activation.service_id)}`, 'DELETE', undefined, true, { 'Idempotency-Key': crypto.randomUUID() })
    if (!result.applied) throw new Error(result.error || 'Deactivation pending; refresh to check applied state')
  })
}
function test(protocol: Protocol) {
  openEditor('Test parser against sample', { sample_id: '', direction: 'client_to_server' }, async input => {
    const result = await api(`${path(protocol)}/test`, 'POST', input)
    showDetail('Test result', result)
  })
}
onMounted(load)
</script>

<template>
  <section class="space-y-4">
    <h1 class="text-2xl font-semibold">Protocols</h1>
    <p class="text-sm text-muted-foreground">Built-in profiles: raw passthrough and bounded HTTP/1.1 requests. Configurable framing emits a typed bytes payload including frame headers/delimiters. Rust WASM parsers use Hydra ABI v1, with fuel and memory limits. Existing flows retain their parser version.</p>
    <div class="flex flex-wrap gap-3"><Button v-if="data?.native_schemas" variant="outline" @click="showDetail('Native parser schemas', data.native_schemas)">Native parser schemas</Button><Button variant="outline" :disabled="busy" @click="framing">Create framing profile</Button><Button variant="outline" :disabled="busy" @click="load">Refresh</Button></div>
    <form class="flex flex-wrap items-end gap-3" @submit.prevent="upload">
      <div class="space-y-2"><Label for="protocol-id">Protocol ID</Label><Input id="protocol-id" v-model="protocolId" required /></div>
      <div class="space-y-2"><Label for="protocol-version">Version</Label><Input id="protocol-version" v-model="version" required /></div>
      <div class="space-y-2"><Label for="protocol-file">WASM module</Label><Input id="protocol-file" type="file" accept=".wasm" required @change="selectFile" /></div>
      <Button type="submit" :disabled="busy">Upload</Button>
    </form>
    <p class="text-sm text-muted-foreground">Framing kinds: raw; delimiter with byte array; fixed_size with size; length_prefix with offset, width (1/2/4/8), little_endian, includes_header. max_frame is 1–8192 bytes. Length excludes the entire header unless includes_header is true. Create a new version to edit.</p>
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p><p v-if="busy" role="status">Loading protocols…</p>
    <p v-else-if="data && !data.items.length" class="text-muted-foreground">No protocol versions uploaded.</p>
    <DataTable v-if="data?.items.length" :columns="columns" :rows="data.items">
      <template #cell-state="{ row }">{{ row.error || row.state }}</template>
      <template #cell-created_at="{ row }">{{ date(row.created_at) }}</template>
      <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2">
        <Select v-model="targets[key(row)]"><SelectTrigger class="w-52" :aria-label="'Service for ' + row.id + ' ' + row.version"><SelectValue placeholder="Select a service" /></SelectTrigger><SelectContent><SelectItem v-for="service in services" :key="service.id" :value="service.id">{{ service.listen_address }}</SelectItem></SelectContent></Select>
        <Button variant="outline" size="sm" :disabled="busy || !targets[key(row)]" @click="activate(row)">Activate</Button>
        <Button variant="outline" size="sm" @click="showDetail('Schema / profile', { profile: row.profile, schema: row.metadata || { note: 'Legacy WASM parser has no declared schema' } })">Schema / profile</Button>
        <Button variant="outline" size="sm" :disabled="busy" @click="test(row)">Test saved sample</Button>
      </div></template>
    </DataTable>
    <template v-if="data?.activations.length">
      <h2 class="text-xl font-semibold">Activations</h2>
      <DataTable :columns="activationColumns" :rows="data.activations">
        <template #cell-applied="{ row }">{{ row.error || String(row.applied) }}</template>
        <template #cell-actions="{ row }"><Button variant="outline" size="sm" :disabled="busy" @click="deactivate(row)">Use service parser</Button></template>
      </DataTable>
    </template>
    <Dialog v-model:open="detailOpen"><DialogContent class="max-h-[85vh] overflow-auto sm:max-w-3xl"><DialogHeader><DialogTitle>{{ detailTitle }}</DialogTitle><DialogDescription>Protocol schema, profile or saved sample evaluation details.</DialogDescription></DialogHeader><pre class="whitespace-pre-wrap break-words text-sm">{{ detailText }}</pre></DialogContent></Dialog>
  </section>
</template>

<script setup lang="ts">
import {username} from '@/shared/users/directory'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { FilterInput, ListResponse } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { enforceSavedFilter } from './filterWorkflow'
import FilterTestDialog from './FilterTestDialog.vue'
import FilterEditorDialog, { type FilterRecord } from './FilterEditorDialog.vue'
import type { DecoderCatalog } from './fieldCatalog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import FilterRuleCard from './FilterRuleCard.vue'
import { setFilterEnabled, type FilterRule } from './filterState'
import { createPendingReconciliation } from './pendingReconciliation'
import { defaultServiceColor, resolveServiceColors } from '@/features/services/colors'
import { parseStreamSelection, type StreamSelection } from '@/features/traffic/streamSelection'

type Filter = FilterRule
type Filters = Omit<ListResponse, 'items'> & { items: Filter[] }
const data = ref<Filters | null>(null)
const services = ref<Array<{ id: string; name: string; transport: string; protocol_id: string; color: string | null }>>([])
const decoderCatalog = ref<DecoderCatalog>({ items: [], activations: [], native_schemas: [] })
const fromTraffic = ref<StreamSelection|null>(null)
const busy = ref(false)
const savingState = ref('')
const serviceColors = computed(() => resolveServiceColors(services.value))
const serviceNames = computed(() => Object.fromEntries(services.value.map(service => [service.id, service.name])))
const editorOpen = ref(false)
watch(editorOpen, open => { if (!open) fromTraffic.value = null })
const editing = ref<Filter | undefined>()
const testOpen = ref(false)
const testing = ref<Pick<FilterRecord, 'id' | 'name' | 'service_id'>>()
const error = ref('')
const notice = ref('')
const query = ref('')
const after = ref('')
const sort = ref('created_at')
const selected = ref<string[]>([])
const reconciliationError = ref('')
let loadedQuery = ''
const reconciliation = createPendingReconciliation({
  pending: () => data.value?.apply_status === 'pending',
  paused: () => busy.value || document.visibilityState === 'hidden',
  read: () => api<Filters>(`/filters?q=${encodeURIComponent(loadedQuery)}&after=${encodeURIComponent(after.value)}`),
  accept: filters => { data.value = filters; reconciliationError.value = '' },
  failed: cause => { reconciliationError.value = `Application status unavailable: ${errorMessage(cause)}` },
})
onBeforeUnmount(() => reconciliation.stop())
const sortOptions = [{ key: 'created_at', label: 'Creation date' }, { key: 'created_by', label: 'Author' }, { key: 'hit_count', label: 'Hits' }, { key: 'priority', label: 'Priority' }, { key: 'name', label: 'Name' }]
const rows = computed(() => [...(data.value?.items ?? [])].sort((a, b) => {
  switch (sort.value) {
    case 'priority': return b.priority - a.priority
    case 'hit_count': return b.hit_count - a.hit_count
    case 'created_by': return username(a.created_by).localeCompare(username(b.created_by))
    case 'name': return a.name.localeCompare(b.name)
    default: return a.created_at.localeCompare(b.created_at)
  }
}))
async function load(cursor = after.value) {
  reconciliation.invalidate()
  const filterQuery = query.value
  busy.value = true
  error.value = ''
  try {
    const [filters, serviceResponse, protocols] = await Promise.all([
      api<Filters>(`/filters?q=${encodeURIComponent(filterQuery)}&after=${encodeURIComponent(cursor)}`),
      api<ListResponse>('/services'),
      api<DecoderCatalog>('/protocols'),
    ])
    data.value = filters
    loadedQuery = filterQuery
    reconciliationError.value = ''
    decoderCatalog.value = protocols
    services.value = serviceResponse.items.flatMap(item => typeof item.id === 'string' ? [{ id: item.id, name: typeof item.name === 'string' ? item.name : item.id, transport: typeof item.transport === 'string' ? item.transport : 'tcp', protocol_id: typeof item.protocol_id === 'string' ? item.protocol_id : 'raw', color: typeof item.color === 'string' ? item.color : null }] : [])
    after.value = cursor
    selected.value = []
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false; reconciliation.resume() }
}
function edit(filter?: Filter) {
  editing.value = filter
  editorOpen.value = true
}
async function saveFilter(value: Omit<FilterInput, 'expected_revision'>) {
  reconciliation.invalidate()
  busy.value = true
  error.value = ''
  try {
    const current = await api<ListResponse>('/filters')
    await api(`/filters${editing.value ? '/' + encodeURIComponent(editing.value.id) : ''}`, editing.value ? 'PATCH' : 'POST', { ...value, expected_revision: current.revision })
    editorOpen.value = false
    notice.value = value.mode === 'enforce' ? 'Saved. Enforcement is requested; check the applied revision.' : 'Saved in Shadow (observe only).'
    await load('')
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false; reconciliation.resume() }
}
async function mutate(action: () => Promise<unknown>) {
  reconciliation.invalidate()
  busy.value = true
  error.value = ''
  try { await action(); await load() }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false; reconciliation.resume() }
}
function toggleSelection(id: string, checked: boolean | 'indeterminate') {
  selected.value = checked === true ? [...new Set([...selected.value, id])] : selected.value.filter(value => value !== id)
}
async function setState(filter: Filter, enabled: boolean) {
  if (busy.value || savingState.value || !data.value || enabled === filter.desired_enabled) return
  const revision = data.value.revision
  savingState.value = filter.id
  try {
    await mutate(() => setFilterEnabled(api, filter.id, enabled, revision))
  } finally { savingState.value = '' }
}
async function bulk(enabled: boolean) {
  if (!selected.value.length || !data.value) { error.value = 'Select filters first'; return }
  const payload = { ids: [...selected.value], enabled, expected_revision: data.value.revision }
  await mutate(() => api('/filters/state', 'PUT', payload))
}
async function remove(filter: Filter) {
  if (!window.confirm('Delete this filter?')) return
  await mutate(async () => {
    const current = await api<ListResponse>('/filters')
    await api(`/filters/${encodeURIComponent(filter.id)}?expected_revision=${current.revision}`, 'DELETE')
  })
}
async function enforce(filter: Filter) {
  await mutate(async () => {
    await enforceSavedFilter(api, filter.id)
    notice.value = 'Enforcement requested. Check the applied revision below.'
  })
}
function test(filter: Filter) {
  testing.value = filter
  testOpen.value = true
}
async function enforced() {
  notice.value = 'Enforcement requested. Check the applied revision below.'
  await load()
}
onMounted(async () => {
  await load('')
  if (new URLSearchParams(window.location.hash.split('?')[1] ?? '').get('createFromTraffic') !== '1') return
  const serialized = sessionStorage.getItem('hydra.filter.stream-selection')
  sessionStorage.removeItem('hydra.filter.stream-selection')
  try { fromTraffic.value = parseStreamSelection(serialized ? JSON.parse(serialized) : null) }
  catch { fromTraffic.value = null }
  if (fromTraffic.value) edit()
})
</script>

<template>
  <section class="space-y-4">
    <FilterEditorDialog v-model:open="editorOpen" :busy="busy" :error="error" :services="services" :catalog="decoderCatalog" :filter="editing" :initial-service-id="fromTraffic?.service_id" :initial-flow-ids="fromTraffic?.flow_ids" @save="saveFilter" />
    <FilterTestDialog v-model:open="testOpen" :filter="testing" @enforced="enforced" />
    <h1 class="text-2xl font-semibold">Filters</h1>
    <div class="flex flex-wrap items-center gap-3">
      <Input v-model="query" :disabled="busy" class="max-w-xs" placeholder="Search filters" aria-label="Search filters" @change="load('')" />
      <p v-if="data" class="text-sm text-muted-foreground">{{ data.apply_status }} · desired {{ data.revision }} / applied {{ data.applied_revision }}</p>
      <Button :disabled="busy" @click="edit()">Add filter</Button><Button variant="outline" :disabled="busy" @click="load()">Refresh</Button>
      <Select v-model="sort"><SelectTrigger class="w-44" aria-label="Sort filters"><SelectValue /></SelectTrigger><SelectContent><SelectItem v-for="option in sortOptions" :key="option.key" :value="option.key">{{ option.label }}</SelectItem></SelectContent></Select>
      <Button variant="outline" :disabled="busy || !selected.length" @click="bulk(true)">Enable selected</Button><Button variant="outline" :disabled="busy || !selected.length" @click="bulk(false)">Disable selected</Button>
      <Button variant="outline" :disabled="busy || !data?.items.length" @click="selected = data?.items.map(item => item.id) ?? []">Select page</Button>
    </div>
    <p v-if="error || reconciliationError || data?.apply_error" role="alert" class="text-sm text-destructive">{{ error || reconciliationError || data?.apply_error }}</p><p v-if="notice" role="status">{{ notice }}</p>
    <p v-if="busy" role="status">Loading filters…</p>
    <p v-else-if="data && !data.items.length" class="text-muted-foreground">No filters. New enabled filters enforce as soon as the proxy applies them. Shadow mode and saved-traffic testing are optional.</p>
    <div v-if="data?.items.length" aria-label="Filter rules" class="space-y-3">
      <FilterRuleCard v-for="row in rows" :key="row.id" :filter="row" :service-name="serviceNames[row.service_id] ?? row.service_id" :service-color="serviceColors[row.service_id] ?? defaultServiceColor(row.service_id)" :selected="selected.includes(row.id)" :disabled="busy || Boolean(savingState)" :saving="savingState === row.id" :apply-status="reconciliationError ? 'error' : data.apply_status" @select="toggleSelection(row.id, $event)" @state="setState(row, $event)" @edit="edit(row)" @test="test(row)" @enforce="enforce(row)" @remove="remove(row)" />
    </div>
    <div class="flex gap-2"><Button v-if="after" variant="outline" :disabled="busy" @click="load('')">First page</Button><Button v-if="data?.next_cursor" variant="outline" :disabled="busy" @click="load(data.next_cursor)">Next page</Button></div>
  </section>
</template>

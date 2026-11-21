<script setup lang="ts">
import {username} from '@/shared/users/directory'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { FilterInput, ListResponse } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { enforceSavedFilter } from './filterWorkflow'
import FilterTestDialog from './FilterTestDialog.vue'
import FilterEditorDialog, { type FilterRecord, type ScriptRecord } from './FilterEditorDialog.vue'
import type { DecoderCatalog } from './fieldCatalog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import FilterRow from './FilterRow.vue'
import RequestCheckerSettings from './RequestCheckerSettings.vue'
import { setFilterEnabled, type FilterRule } from './filterState'
import { createPendingReconciliation } from './pendingReconciliation'
import { hitsByService, visibleFilters } from './filterRow'
import { assignService, scriptRows as buildScriptRows, setPolicyService, upsertPolicy, withoutPolicy, type ScriptPolicy, type ScriptProgram, type ScriptRow } from './scriptFilters'
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
const editingScript = ref<ScriptRecord | null>(null)
const editorTab = ref<'regex' | 'script'>('regex')
const programs = ref<ScriptProgram[]>([])
const policies = ref<ScriptPolicy[]>([])
const programsRevision = ref(0)
const policiesRevision = ref(0)
// Keep all configured checkers visible in a service view so a disabled checker
// can be re-enabled for that service after its policy assignment is removed.
const scripts = computed<ScriptRow[]>(() => buildScriptRows(programs.value, policies.value))
const testOpen = ref(false)
const testing = ref<Pick<FilterRecord, 'id' | 'name' | 'service_id' | 'additional_service_ids'>>()
const error = ref('')
const notice = ref('')
const query = ref('')
const serviceFilter = ref('')
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
const serviceHits = computed(() => hitsByService(data.value?.items ?? []))
const serviceRows = computed(() => services.value.map(service => ({ ...service, hits: serviceHits.value[service.id] ?? 0, color: serviceColors.value[service.id] ?? defaultServiceColor(service.id) })))
const visible = computed(() => visibleFilters(data.value?.items ?? [], serviceFilter.value))
const rows = computed(() => [...visible.value].sort((a, b) => {
  switch (sort.value) {
    case 'priority': return b.priority - a.priority
    case 'hit_count': return b.hit_count - a.hit_count
    case 'created_by': return username(a.created_by).localeCompare(username(b.created_by))
    case 'name': return a.name.localeCompare(b.name)
    default: return a.created_at.localeCompare(b.created_at)
  }
}))
async function loadServices(): Promise<ListResponse['items']> {
  const first = await api<ListResponse>('/services?limit=100')
  if (!first.next_cursor) return first.items
  // Hydra supports at most 128 services, so there are at most two pages.
  const second = await api<ListResponse>(`/services?limit=100&after=${encodeURIComponent(first.next_cursor)}`)
  return [...first.items, ...second.items]
}
async function load(cursor = after.value) {
  reconciliation.invalidate()
  const filterQuery = query.value
  busy.value = true
  error.value = ''
  try {
    const [filters, serviceItems, protocols, checkerSettings, checkerPrograms] = await Promise.all([
      api<Filters>(`/filters?q=${encodeURIComponent(filterQuery)}&after=${encodeURIComponent(cursor)}`),
      loadServices(),
      api<DecoderCatalog>('/protocols'),
      api<{ revision: number; policies: ScriptPolicy[] }>('/filters/request-checkers').catch(() => ({ revision: 0, policies: [] })),
      api<{ revision: number; programs: ScriptProgram[] }>('/filters/request-checkers/programs').catch(() => ({ revision: 0, programs: [] })),
    ])
    policies.value = checkerSettings.policies
    policiesRevision.value = checkerSettings.revision
    programs.value = checkerPrograms.programs
    programsRevision.value = checkerPrograms.revision
    data.value = filters
    loadedQuery = filterQuery
    reconciliationError.value = ''
    decoderCatalog.value = protocols
    services.value = serviceItems.flatMap(item => typeof item.id === 'string' ? [{ id: item.id, name: typeof item.name === 'string' ? item.name : item.id, transport: typeof item.transport === 'string' ? item.transport : 'tcp', protocol_id: typeof item.protocol_id === 'string' ? item.protocol_id : 'raw', color: typeof item.color === 'string' ? item.color : null }] : [])
    after.value = cursor
    selected.value = []
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false; reconciliation.resume() }
}
function edit(filter?: Filter) {
  editing.value = filter
  editingScript.value = null
  editorTab.value = 'regex'
  editorOpen.value = true
}
function editScript(row: ScriptRow) {
  editing.value = undefined
  editingScript.value = { id: row.id, name: row.id, source: row.source, allowOnError: row.allowOnError, serviceId: row.services[0] ?? services.value[0]?.id ?? '' }
  editorTab.value = 'script'
  editorOpen.value = true
}
async function saveScript(input: { id: string; source: string; serviceId: string; allowOnError: boolean }) {
  busy.value = true
  error.value = ''
  try {
    // The Request checker panel can also edit these programs and policies.
    // Reload its shared revision so a still-open filter page doesn't submit
    // against the snapshot it loaded before those edits.
    const latestSettings = await api<{ revision: number; policies: ScriptPolicy[] }>('/filters/request-checkers')
    policies.value = latestSettings.policies
    policiesRevision.value = latestSettings.revision
    programsRevision.value = latestSettings.revision
    const updatedPrograms = await api<{ revision: number; programs: ScriptProgram[] }>(`/filters/request-checkers/programs/${encodeURIComponent(input.id)}`, 'PUT', { revision: programsRevision.value, source: input.source })
    // Program and service-policy writes share one revision. Save against the
    // revision returned by the program write, which itself advances it.
    programsRevision.value = updatedPrograms.revision
    policiesRevision.value = updatedPrograms.revision
    const existing = policies.value.find(policy => policy.id === input.id)
    const policy: ScriptPolicy = {
      id: input.id,
      services: [...new Set([...(existing?.services ?? []), input.serviceId])],
      timeout_ms: existing?.timeout_ms ?? 50,
      allow_on_error: input.allowOnError,
    }
    await api('/filters/request-checkers', 'PUT', { revision: policiesRevision.value, policies: assignService(upsertPolicy(policies.value, policy), policy.id, input.serviceId) })
    editorOpen.value = false
    notice.value = 'Script saved. The proxy applies it to new requests.'
    await load('')
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
async function savePolicies(next: ScriptPolicy[]) {
  await api('/filters/request-checkers', 'PUT', { revision: policiesRevision.value, policies: next })
  await load('')
}
async function toggleScript(row: ScriptRow, enabled: boolean) {
  const serviceId = serviceFilter.value
  if (!serviceId) return
  const next = enabled
    ? assignService(policies.value, row.id, serviceId)
    : setPolicyService(policies.value, row.id, serviceId, false)
  try { await savePolicies(next) }
  catch (cause) { error.value = errorMessage(cause) }
}
async function removeScript(row: ScriptRow) {
  if (!window.confirm('Delete this script?')) return
  try {
    await savePolicies(withoutPolicy(policies.value, row.id))
    await api(`/filters/request-checkers/programs/${encodeURIComponent(row.id)}`, 'DELETE', { revision: programsRevision.value }).catch(() => {})
  } catch (cause) { error.value = errorMessage(cause) }
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
    <RequestCheckerSettings :services="services" />
    <FilterEditorDialog v-model:open="editorOpen" :busy="busy" :error="error" :services="services" :catalog="decoderCatalog" :filter="editing" :script="editingScript" :initial-tab="editorTab" :initial-service-id="fromTraffic?.service_id" :initial-flow-ids="fromTraffic?.flow_ids" @save="saveFilter" @save-script="saveScript" />
    <FilterTestDialog v-model:open="testOpen" :filter="testing" :service-names="serviceNames" @enforced="enforced" />
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
    <div class="flex items-start gap-3">
      <aside v-if="services.length" aria-label="Services" class="w-56 shrink-0 overflow-hidden rounded-md border">
        <button type="button" class="flex w-full items-center gap-2 border-b px-2 py-2 text-left text-sm" :class="serviceFilter === '' ? 'bg-secondary' : ''" @click="serviceFilter = ''">
          <span class="flex-1 truncate">All services</span><span class="text-xs tabular-nums text-muted-foreground">{{ (data?.items ?? []).length }}</span>
        </button>
        <button v-for="service in serviceRows" :key="service.id" type="button" class="flex w-full items-center gap-2 border-b px-2 py-2 text-left text-sm last:border-b-0" :class="serviceFilter === service.id ? 'bg-secondary' : ''" @click="serviceFilter = service.id">
          <span class="size-2 shrink-0 rounded-full" :style="{ background: service.color }" aria-hidden="true" />
          <span class="flex-1 truncate">{{ service.name }}</span><span class="text-xs tabular-nums text-muted-foreground">{{ service.hits }}</span>
        </button>
      </aside>
      <div class="min-w-0 flex-1 space-y-3">
      <p v-if="data && !rows.length && !scripts.length" class="text-muted-foreground">{{ serviceFilter ? 'No filters for this service.' : 'No filters. New enabled filters enforce as soon as the proxy applies them.' }}</p>
      <div v-if="rows.length || scripts.length" aria-label="Filter rules" class="rounded-md border">
      <FilterRow v-for="row in rows" :key="row.id" :filter="row" :service-name="[row.service_id, ...(row.additional_service_ids ?? [])].map(id => serviceNames[id] ?? id).join(', ')" :service-color="serviceColors[row.service_id] ?? defaultServiceColor(row.service_id)" :selected="selected.includes(row.id)" :disabled="busy || Boolean(savingState)" :saving="savingState === row.id" :apply-status="reconciliationError ? 'error' : data?.apply_status ?? ''" @select="toggleSelection(row.id, $event)" @state="setState(row, $event)" @edit="edit(row)" @test="test(row)" @enforce="enforce(row)" @remove="remove(row)" />
      <div v-for="row in scripts" :key="row.id" class="group border-b border-l-4 last:border-b-0" :style="{ borderLeftColor: serviceColors[serviceFilter || row.services[0] || ''] ?? defaultServiceColor(serviceFilter || row.services[0] || '') }">
        <div class="flex flex-wrap items-center gap-2 px-2 py-0.5 text-[13px]">
          <Switch :model-value="serviceFilter ? row.services.includes(serviceFilter) : row.services.length > 0" :disabled="busy || !serviceFilter" :aria-label="'Enable ' + row.name" @update:model-value="toggleScript(row, $event)" />
          <span class="rounded border px-1.5 text-[11px] leading-5 text-muted-foreground">python</span>
          <code class="max-w-80 truncate font-mono">{{ row.name }}</code>
          <span class="rounded border px-1.5 text-[11px] leading-5 text-muted-foreground">c→s</span>
          <span class="text-[11px] text-muted-foreground">{{ row.services.map(id => serviceNames[id] ?? id).join(', ') }}</span>
          <span class="rounded border px-1.5 text-[11px] leading-5 text-muted-foreground">on error: {{ row.allowOnError ? 'allow' : 'block' }}</span>
          <div class="ml-auto flex items-center gap-1">
            <Button variant="ghost" size="xs" :disabled="busy" @click="editScript(row)">Edit</Button>
            <Button variant="ghost" size="xs" class="text-destructive" :disabled="busy" @click="removeScript(row)">Delete</Button>
          </div>
        </div>
        <pre class="hidden overflow-x-auto border-t px-2 py-1 font-mono text-[11px] leading-5 text-muted-foreground group-hover:block">{{ row.source }}</pre>
      </div>
      </div>
      </div>
    </div>
    <div class="flex gap-2"><Button v-if="after" variant="outline" :disabled="busy" @click="load('')">First page</Button><Button v-if="data?.next_cursor" variant="outline" :disabled="busy" @click="load(data.next_cursor)">Next page</Button></div>
  </section>
</template>

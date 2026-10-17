<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import type { FilterInput, ListResponse } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { date } from '@/shared/format'
import FilterTestDialog from './FilterTestDialog.vue'
import FilterEditorDialog, { type FilterRecord } from './FilterEditorDialog.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import DataTable from '@/components/DataTable.vue'

type Filter = FilterRecord & { updated_at: string; updated_by: string; desired_revision: number; applied_revision: number; desired_enabled: boolean; applied_enabled: boolean | null }
type Filters = Omit<ListResponse, 'items'> & { items: Filter[] }
const data = ref<Filters | null>(null)
const services = ref<Array<{ id: string; name: string; transport: string }>>([])
const busy = ref(false)
const editorOpen = ref(false)
const editing = ref<Filter | undefined>()
const testOpen = ref(false)
const testing = ref<Pick<FilterRecord, 'id' | 'name' | 'service_id'>>()
const error = ref('')
const notice = ref('')
const query = ref('')
const after = ref('')
const sort = ref('created_at')
const selected = ref<string[]>([])
const columns = [{ key: 'select', label: 'Select' }, { key: 'name', label: 'Name / mode' }, { key: 'priority', label: 'Priority' }, { key: 'created_at', label: 'Created' }, { key: 'author', label: 'Author / editor' }, { key: 'hit_count', label: 'Hits' }, { key: 'enabled', label: 'Enabled' }, { key: 'actions', label: 'Actions' }]
const sortOptions = [{ key: 'created_at', label: 'Creation date' }, { key: 'created_by', label: 'Author' }, { key: 'hit_count', label: 'Hits' }, { key: 'priority', label: 'Priority' }, { key: 'name', label: 'Name' }]
const rows = computed(() => [...(data.value?.items ?? [])].sort((a, b) => {
  switch (sort.value) {
    case 'priority': return b.priority - a.priority
    case 'hit_count': return b.hit_count - a.hit_count
    case 'created_by': return a.created_by.localeCompare(b.created_by)
    case 'name': return a.name.localeCompare(b.name)
    default: return a.created_at.localeCompare(b.created_at)
  }
}))
async function load(cursor = after.value) {
  busy.value = true
  error.value = ''
  try {
    const [filters, serviceResponse] = await Promise.all([api<Filters>(`/filters?q=${encodeURIComponent(query.value)}&after=${encodeURIComponent(cursor)}`), api<ListResponse>('/services')])
    data.value = filters
    services.value = serviceResponse.items.flatMap(item => typeof item.id === 'string' ? [{ id: item.id, name: typeof item.name === 'string' ? item.name : item.id, transport: typeof item.transport === 'string' ? item.transport : 'tcp' }] : [])
    after.value = cursor
    selected.value = []
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
function edit(filter?: Filter) {
  editing.value = filter
  editorOpen.value = true
}
async function saveFilter(value: Omit<FilterInput, 'expected_revision'>) {
  busy.value = true
  error.value = ''
  try {
    const current = await api<ListResponse>('/filters')
    const needsTest = value.mode === 'enforce'
    const saved = await api<{ rules: { rules: Array<{ id: string }> } }>(`/filters${editing.value ? '/' + encodeURIComponent(editing.value.id) : ''}`, editing.value ? 'PATCH' : 'POST', { ...value, mode: needsTest ? 'shadow' : value.mode, expected_revision: current.revision })
    // Create appends its new rule to the returned desired snapshot.
    const id = editing.value?.id ?? saved.rules.rules.at(-1)?.id
    editorOpen.value = false
    await load('')
    if (needsTest && id) {
      testing.value = { id, name: value.name, service_id: value.service_id }
      testOpen.value = true
      notice.value = 'Saved in Shadow. Select captured traffic to test and enforce your changes.'
    }
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
async function mutate(action: () => Promise<unknown>) {
  busy.value = true
  error.value = ''
  try { await action(); await load() }
  catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}
function toggleSelection(id: string, checked: boolean | 'indeterminate') {
  selected.value = checked === true ? [...new Set([...selected.value, id])] : selected.value.filter(value => value !== id)
}
async function setState(filter: Filter) {
  await mutate(async () => {
    const current = await api<ListResponse>('/filters')
    await api(`/filters/${encodeURIComponent(filter.id)}/state`, 'PUT', { enabled: !filter.enabled, expected_revision: current.revision })
  })
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
function test(filter: Filter) {
  testing.value = filter
  testOpen.value = true
}
async function enforced() {
  notice.value = 'Enforcement requested. Check the applied revision below.'
  await load()
}
onMounted(() => load(''))
</script>

<template>
  <section class="space-y-4">
    <FilterEditorDialog v-model:open="editorOpen" :busy="busy" :error="error" :services="services" :filter="editing" @save="saveFilter" />
    <FilterTestDialog v-model:open="testOpen" :filter="testing" @enforced="enforced" />
    <h1 class="text-2xl font-semibold">Filters</h1>
    <div class="flex flex-wrap items-center gap-3">
      <Input v-model="query" class="max-w-xs" placeholder="Search filters" aria-label="Search filters" @change="load('')" />
      <p v-if="data" class="text-sm text-muted-foreground">{{ data.apply_status }} · desired {{ data.revision }} / applied {{ data.applied_revision }}</p>
      <Button :disabled="busy" @click="edit()">Add filter</Button><Button variant="outline" :disabled="busy" @click="load()">Refresh</Button>
      <Select v-model="sort"><SelectTrigger class="w-44" aria-label="Sort filters"><SelectValue /></SelectTrigger><SelectContent><SelectItem v-for="option in sortOptions" :key="option.key" :value="option.key">{{ option.label }}</SelectItem></SelectContent></Select>
      <Button variant="outline" :disabled="busy || !selected.length" @click="bulk(true)">Enable selected</Button><Button variant="outline" :disabled="busy || !selected.length" @click="bulk(false)">Disable selected</Button>
      <Button variant="outline" :disabled="busy || !data?.items.length" @click="selected = data?.items.map(item => item.id) ?? []">Select page</Button>
    </div>
    <p v-if="error || data?.apply_error" role="alert" class="text-sm text-destructive">{{ error || data?.apply_error }}</p><p v-if="notice" role="status">{{ notice }}</p>
    <p v-if="busy" role="status">Loading filters…</p>
    <p v-else-if="data && !data.items.length" class="text-muted-foreground">No filters. Start in shadow mode, inspect traffic, then test a saved message before enforcing.</p>
    <DataTable v-if="data?.items.length" :columns="columns" :rows="rows">
      <template #cell-select="{ row }"><Checkbox :model-value="selected.includes(row.id)" :aria-label="'Select ' + row.name" @update:model-value="value => toggleSelection(row.id, value)" /></template>
      <template #cell-name="{ row }"><div>{{ row.name }}</div><p class="text-sm" :class="row.mode === 'shadow' ? 'font-medium' : ''">{{ row.mode === 'shadow' ? 'Shadow — observes only; traffic passes' : 'Enforce — applies action to matches' }}</p></template>
      <template #cell-created_at="{ row }">{{ date(row.created_at) }}</template>
      <template #cell-author="{ row }"><div>{{ row.created_by }}</div><div class="text-muted-foreground">{{ row.updated_by }}</div></template>
      <template #cell-enabled="{ row }"><Button variant="outline" size="sm" :disabled="busy" @click="setState(row)">{{ row.enabled ? 'Turn off' : 'Turn on' }}</Button><p class="text-sm">Desired {{ row.desired_enabled ? 'on' : 'off' }} / applied {{ row.applied_enabled === null ? 'pending' : row.applied_enabled ? 'on' : 'off' }}</p></template>
      <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2"><Button variant="outline" size="sm" :disabled="busy" @click="edit(row)">Edit</Button><Button variant="outline" size="sm" :disabled="busy" @click="test(row)">Test saved message</Button><Button v-if="row.mode === 'shadow'" variant="outline" size="sm" :disabled="busy" @click="test(row)">Test and enforce</Button><Button variant="destructive" size="sm" :disabled="busy" @click="remove(row)">Delete</Button></div></template>
    </DataTable>
    <div class="flex gap-2"><Button v-if="after" variant="outline" :disabled="busy" @click="load('')">First page</Button><Button v-if="data?.next_cursor" variant="outline" :disabled="busy" @click="load(data.next_cursor)">Next page</Button></div>
  </section>
</template>

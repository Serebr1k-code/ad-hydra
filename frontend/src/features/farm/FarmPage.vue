<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import type { Exploit, Target } from '@/generated/contracts'
import type { FarmKind, FarmPage, FarmRun } from './types'
import { api, errorMessage } from '@/shared/api/client'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import TargetForm from './TargetForm.vue'
import ExploitForm from './ExploitForm.vue'
import FarmRuns from './FarmRuns.vue'
const empty = <T,>(): FarmPage<T> => ({ items: [], next_cursor: null })
const targets = ref(empty<Target>()), exploits = ref(empty<Exploit>()), runs = ref(empty<FarmRun>())
const selectedTarget = ref<Target | null>(null), selectedExploit = ref<Exploit | null>(null)
const cursors = reactive<Record<FarmKind, string>>({ targets: '', exploits: '', runs: '' })
const busy = ref(false), loading = ref(true), error = ref(''), detail = ref(''), detailTitle = ref('')
const keys = new Map<string, string>()
const targetColumns = [{ key: 'name', label: 'Target' }, { key: 'address', label: 'Address' }, { key: 'transport', label: 'Transport' }, { key: 'enabled', label: 'Enabled' }, { key: 'actions', label: 'Actions' }]
const exploitColumns = [{ key: 'name', label: 'Exploit' }, { key: 'version', label: 'Version' }, { key: 'enabled', label: 'Enabled' }, { key: 'schedule', label: 'Schedule' }, { key: 'actions', label: 'Actions' }]
const query = (kind: FarmKind) => cursors[kind] ? `?after=${encodeURIComponent(cursors[kind])}` : ''
async function load() {
  loading.value = true
  try {
    const result = await Promise.all([api<FarmPage<Target>>(`/farm/targets${query('targets')}`), api<FarmPage<Exploit>>(`/farm/exploits${query('exploits')}`), api<FarmPage<FarmRun>>(`/farm/runs${query('runs')}`)])
    ;[targets.value, exploits.value, runs.value] = result
  } finally { loading.value = false }
}
async function action(work: () => Promise<unknown>, reload = true) {
  if (busy.value) return
  busy.value = true; error.value = ''
  try { await work(); if (reload) await load() }
  catch (e) { error.value = errorMessage(e) }
  finally { busy.value = false }
}
function saved(kind: 'targets' | 'exploits') {
  if (kind === 'targets') selectedTarget.value = null
  else selectedExploit.value = null
  void action(load, false)
}
function toggleTarget(t: Target) {
  void action(() => api('/farm/targets', 'PUT', { id: t.id, name: t.name, host: t.host, port: t.port, transport: t.transport, enabled: !t.enabled, revision: t.revision }))
}
function toggleExploit(e: Exploit) {
  void action(() => api('/farm/exploits', 'PUT', { id: e.id, name: e.name, version: e.version, enabled: !e.enabled, revision: e.revision, manifest: e.manifest, target_ids: e.target_ids, interval_seconds: e.interval_seconds ?? null }))
}
function remove(kind: 'targets' | 'exploits', row: Target | Exploit) {
  void action(() => api(`/farm/${kind}/${encodeURIComponent(row.id)}?revision=${row.revision}`, 'DELETE'))
}
function enqueue(e: Exploit) { void idempotent(`run:${e.id}:${e.revision}`, '/farm/runs', { exploit_id: e.id, revision: e.revision }) }
async function idempotent(operation: string, path: string, body: unknown = null) {
  await action(async () => {
    const key = keys.get(operation) ?? crypto.randomUUID()
    keys.set(operation, key)
    await api(path, 'POST', body, true, { 'Idempotency-Key': key })
    keys.delete(operation)
  })
}
function retry(run: FarmRun) { void idempotent(`retry:${run.id}`, `/farm/runs/${encodeURIComponent(run.id)}/retry`) }
function cancel(run: FarmRun) { void action(() => api(`/farm/runs/${encodeURIComponent(run.id)}/cancel`, 'POST')) }
function showDetail(title: string, path: string) {
  void action(async () => { const value = await api<unknown>(path); detail.value = JSON.stringify(value, null, 2) ?? ''; detailTitle.value = title }, false)
}
function next(kind: FarmKind) {
  const page = kind === 'targets' ? targets.value : kind === 'exploits' ? exploits.value : runs.value
  if (!page.next_cursor) return
  cursors[kind] = page.next_cursor
  void action(load, false)
}
function first() {
  cursors.targets = ''; cursors.exploits = ''; cursors.runs = ''
  void action(load, false)
}
onMounted(() => { void action(load, false) })
</script>
<template>
  <div class="space-y-6">
    <header><h1 class="text-2xl font-semibold">Exploit farm</h1><p class="text-muted-foreground">Enabled numeric-IP competition targets only. Entrypoints are immutable, limited to 2 MiB, and run in the configured sandbox. One worker; bounded logs.</p></header>
    <p v-if="error" role="alert" class="text-destructive">{{ error }}</p>
    <p v-if="loading" role="status">Loading farm…</p>
    <TargetForm :target="selectedTarget" @saved="saved('targets')" @cancel="selectedTarget = null" />
    <section class="space-y-3"><h2 class="text-lg font-semibold">Target allowlist</h2>
      <DataTable :columns="targetColumns" :rows="targets.items">
        <template #cell-address="{ row }">{{ row.host }}:{{ row.port }}</template>
        <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2"><Button variant="outline" :disabled="busy" @click="selectedTarget = row">Edit</Button><Button variant="outline" :disabled="busy" @click="toggleTarget(row)">{{ row.enabled ? 'Disable' : 'Enable' }}</Button><Button variant="outline" :disabled="busy" @click="remove('targets', row)">Delete</Button></div></template>
      </DataTable>
      <Button v-if="targets.items.length === 100 && targets.next_cursor" variant="outline" :disabled="busy" @click="next('targets')">Next targets</Button>
    </section>
    <ExploitForm :exploit="selectedExploit" :targets="targets.items" @saved="saved('exploits')" @cancel="selectedExploit = null" />
    <section class="space-y-3"><h2 class="text-lg font-semibold">Exploits</h2>
      <DataTable :columns="exploitColumns" :rows="exploits.items">
        <template #cell-schedule="{ row }">{{ row.interval_seconds ? `${row.interval_seconds}s` : 'Manual' }}</template>
        <template #cell-actions="{ row }"><div class="flex flex-wrap gap-2"><Button variant="outline" :disabled="busy" @click="selectedExploit = row">Edit</Button><Button variant="outline" :disabled="busy" @click="toggleExploit(row)">{{ row.enabled ? 'Disable' : 'Enable' }}</Button><Button variant="outline" :disabled="busy" @click="showDetail(`Versions: ${row.name}`, `/farm/exploits/${encodeURIComponent(row.id)}/versions`)">Versions</Button><Button variant="outline" :disabled="busy || !row.enabled" @click="enqueue(row)">Run</Button><Button variant="outline" :disabled="busy" @click="remove('exploits', row)">Delete</Button></div></template>
      </DataTable>
      <Button v-if="exploits.items.length === 100 && exploits.next_cursor" variant="outline" :disabled="busy" @click="next('exploits')">Next exploits</Button>
    </section>
    <FarmRuns :runs="runs.items" :busy="busy" @refresh="action(load, false)" @logs="run => showDetail(`Run: ${run.id}`, `/farm/runs/${encodeURIComponent(run.id)}`)" @cancel="cancel" @retry="retry" />
    <div class="flex gap-2"><Button v-if="runs.items.length === 100 && runs.next_cursor" variant="outline" :disabled="busy" @click="next('runs')">Next runs</Button><Button v-if="Object.values(cursors).some(Boolean)" variant="outline" :disabled="busy" @click="first">First pages</Button></div>
    <section v-if="detail" class="space-y-2"><div class="flex items-center justify-between"><h2 class="text-lg font-semibold">{{ detailTitle }}</h2><Button variant="outline" @click="detail = ''">Close details</Button></div><pre class="max-h-96 overflow-auto rounded border p-3 text-sm">{{ detail }}</pre></section>
  </div>
</template>

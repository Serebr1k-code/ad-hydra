<script setup lang="ts">
import {identityMetadata} from '@/shared/users/directory'
import { onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import type { Exploit, FarmService, FarmTeam } from '@/generated/contracts'
import type { FarmPage, FarmRun, FarmRunDetail } from './types'
import { api, errorMessage } from '@/shared/api/client'
import { catalog } from './farmApi'
import { exploitDto } from './farmDto'
import DisclosureSection from '@/components/DisclosureSection.vue'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import FarmCatalog from './FarmCatalog.vue'
import ExploitForm from './ExploitForm.vue'
import FarmRuns from './FarmRuns.vue'
import FarmEffectiveness from './FarmEffectiveness.vue'
import FarmRunDetailView from './FarmRunDetail.vue'
import {Dialog,DialogContent,DialogHeader,DialogTitle,DialogDescription} from '@/components/ui/dialog'
const empty = <T,>(): FarmPage<T> => ({ items: [], next_cursor: null })
const services = ref<FarmService[]>([]), teams = ref<FarmTeam[]>([])
const exploits = ref(empty<Exploit>()), runs = ref(empty<FarmRun>())
const runsOpen=ref(false)
const selectedExploit = ref<Exploit | null>(null), selectedTeams = reactive<Record<string, string>>({})
const cursors = reactive({ exploits: '', runs: '' })
const busy = ref(false), loading = ref(true), error = ref(''), notice = ref('')
const detail = ref<FarmRunDetail | null>(null), detailLoading = ref(false), requestedDetail = ref('')
const versionDetail = ref(''), versionTitle = ref('')
const keys = new Map<string, string>()
let detailRefreshUntil = 0
let logTrigger:HTMLElement|null=null
let mounted = true, polling = false, timer: ReturnType<typeof setInterval> | undefined
const exploitColumns = [{ key: 'name', label: 'Exploit' }, { key: 'service', label: 'Service' }, { key: 'enabled', label: 'Enabled' }, { key: 'schedule', label: 'Schedule' }, { key: 'actions', label: 'Actions' }]
const query = (kind: 'exploits' | 'runs') => cursors[kind] ? `?after=${encodeURIComponent(cursors[kind])}` : ''
async function load() {
  const result = await Promise.all([catalog<FarmService>('services'), catalog<FarmTeam>('teams'), api<FarmPage<Exploit>>(`/farm/exploits${query('exploits')}`), api<FarmPage<FarmRun>>(`/farm/runs${query('runs')}`)])
  if (mounted) [services.value, teams.value, exploits.value, runs.value] = result
}
async function action(work: () => Promise<unknown>, reload = true) {
  if (busy.value) return
  busy.value = true; error.value = ''; notice.value = ''
  try { await work(); if (reload) await load() }
  catch (e) { if (mounted) error.value = errorMessage(e) }
  finally { busy.value = false; loading.value = false }
}
function saved() { selectedExploit.value = null; void action(load, false) }
function toggleExploit(e: Exploit) { void action(() => api('/farm/exploits', 'PUT', exploitDto({ ...e, enabled: !e.enabled }))) }
function remove(e: Exploit) { void action(() => api(`/farm/exploits/${encodeURIComponent(e.id)}?revision=${e.revision}`, 'DELETE')) }
function allowedTeams(exploit: Exploit) { return teams.value.filter(team => team.enabled && (!(exploit.team_ids?.length) || exploit.team_ids.includes(team.id))) }
function enqueue(e: Exploit) {
  const team = selectedTeams[e.id] || 'all'
  void idempotent(`run:${e.id}:${e.revision}:${team}`, '/farm/runs', { exploit_id: e.id, revision: e.revision, ...(team === 'all' ? {} : { team_id: team }) }, true)
}
async function idempotent(operation: string, path: string, body: unknown = null, firstRuns = false) {
  await action(async () => {
    const key = keys.get(operation) ?? crypto.randomUUID()
    keys.set(operation, key)
    await api(path, 'POST', body, true, { 'Idempotency-Key': key })
    keys.delete(operation)
    if (firstRuns) { cursors.runs = ''; runsOpen.value=true }
  })
}
function retry(run: FarmRun) { void idempotent(`retry:${run.id}`, `/farm/runs/${encodeURIComponent(run.id)}/retry`, null, true) }
function cancel(run: FarmRun) { void action(() => api(`/farm/runs/${encodeURIComponent(run.id)}/cancel`, 'POST')) }
async function readDetail(id: string) {
  detailLoading.value = true
  try {
    const value = await api<FarmRunDetail>(`/farm/runs/${encodeURIComponent(id)}`)
    if (mounted && requestedDetail.value === id) {
      detail.value = value
      if (['queued', 'running'].includes(value.status)) detailRefreshUntil = Date.now() + 10000
    }
  } finally { detailLoading.value = false }
}
function showLogs(run: Pick<FarmRun, 'id'>) { logTrigger=document.activeElement instanceof HTMLElement?document.activeElement:null;detail.value=null;detailRefreshUntil = Date.now() + 10000; requestedDetail.value = run.id; void action(() => readDetail(run.id), false) }
function closeLogs() { requestedDetail.value = ''; detail.value = null }
function restoreLogFocus(event:Event){if(logTrigger?.isConnected){event.preventDefault();logTrigger.focus()}}
function showVersions(e: Exploit) {
  void action(async () => { const value = await api<unknown>(`/farm/exploits/${encodeURIComponent(e.id)}/versions`); versionDetail.value = JSON.stringify(identityMetadata(value), null, 2) ?? ''; versionTitle.value = `Versions: ${e.name}` }, false)
}
function next(kind: 'exploits' | 'runs') {
  const page = kind === 'exploits' ? exploits.value : runs.value
  if (!page.next_cursor) return
  cursors[kind] = page.next_cursor
  void action(load, false)
}
function first(kind: 'exploits' | 'runs') { cursors[kind] = ''; void action(load, false) }
function cleanup() {
  void action(async () => {
    const result = await api<{ deleted: number }>('/farm/runs?older_than_days=7', 'DELETE')
    cursors.runs = ''; notice.value = `Removed ${result.deleted} completed runs under the retention policy (7 days, 1,000 runs, 64 MiB compressed).`
  })
}
async function poll() {
  if (!mounted || polling || busy.value || (!runsOpen.value && !requestedDetail.value)) return
  polling = true
  try {
    const cursor = cursors.runs
    const result = await api<FarmPage<FarmRun>>(`/farm/runs${query('runs')}`)
    if (mounted && cursor === cursors.runs) runs.value = result
    // Cancellation is terminal before the worker finishes draining its output pipes.
    if (detail.value && (['queued', 'running'].includes(detail.value.status) || Date.now() < detailRefreshUntil)) await readDetail(detail.value.id)
  } catch (e) { if (mounted) error.value = errorMessage(e) }
  finally { polling = false }
}
watch(runsOpen,open=>{if(open)void action(load,false)})
onMounted(() => { void action(load, false); timer = setInterval(() => { void poll() }, 2000) })
onUnmounted(() => { mounted = false; if (timer) clearInterval(timer) })
</script>
<template>
  <div class="space-y-6">
    <header><h1 class="text-2xl font-semibold">Exploit farm</h1><p class="text-muted-foreground">Add services and authorized teams, then upload an exploit and run it against one team or all selected teams.</p></header>
    <div class="flex flex-wrap items-center gap-3"><Button as-child variant="outline"><RouterLink to="/flags">Flag queue / submitter</RouterLink></Button><p class="text-sm text-muted-foreground">The configured flag regex extracts matches from Farm stdout and stderr and queues captures automatically.</p></div>
    <p v-if="error" role="alert" class="text-destructive">{{ error }}</p><p v-if="notice" role="status">{{ notice }}</p><p v-if="loading" role="status">Loading farm…</p>
    <FarmCatalog kind="services" :items="services" @saved="action(load, false)" />
    <FarmCatalog kind="teams" :items="teams" @saved="action(load, false)" />
    <ExploitForm :exploit="selectedExploit" :services="services" :teams="teams" @saved="saved" @cancel="selectedExploit = null" />
    <section class="space-y-3"><h2 class="text-lg font-semibold">Exploits</h2>
      <DataTable :columns="exploitColumns" :rows="exploits.items">
        <template #cell-service="{ row }">{{ services.find(s => s.id === row.service_id)?.name ?? (row.service_id || 'Legacy targets') }}</template>
        <template #cell-schedule="{ row }">{{ row.interval_seconds ? `${row.interval_seconds}s` : 'Manual' }}</template>
        <template #cell-actions="{ row }"><div class="flex flex-wrap items-center gap-2"><Button variant="outline" :disabled="busy" @click="selectedExploit = row">Edit</Button><Button variant="outline" :disabled="busy" @click="toggleExploit(row)">{{ row.enabled ? 'Disable' : 'Enable' }}</Button><Button variant="outline" :disabled="busy" @click="showVersions(row)">Versions</Button>
          <div v-if="row.service_id"><Label class="sr-only" :for="`run-team-${row.id}`">Team for {{ row.name }}</Label><Select :model-value="selectedTeams[row.id] || 'all'" @update:model-value="selectedTeams[row.id] = String($event)" :disabled="busy"><SelectTrigger :id="`run-team-${row.id}`" class="w-44"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">All selected teams</SelectItem><SelectItem v-for="team in allowedTeams(row)" :key="team.id" :value="team.id">{{ team.name }}</SelectItem></SelectContent></Select></div>
          <Button variant="outline" :disabled="busy || !row.enabled" @click="enqueue(row)">Run</Button><Button variant="outline" :disabled="busy" @click="remove(row)">Delete</Button></div></template>
      </DataTable>
      <div class="flex gap-2"><Button v-if="cursors.exploits" variant="outline" :disabled="busy" @click="first('exploits')">First exploits</Button><Button v-if="exploits.items.length === 100 && exploits.next_cursor" variant="outline" :disabled="busy" @click="next('exploits')">Next exploits</Button></div>
    </section>
    <FarmEffectiveness :services="services" @logs="id=>showLogs({id})" />
    <DisclosureSection v-model:open="runsOpen" title="Run history" :description="`${runs.items.length} runs on this page`">
    <FarmRuns :runs="runs.items" :busy="busy" @refresh="action(load, false)" @logs="showLogs" @cancel="cancel" @retry="retry" />
    <p class="text-sm text-muted-foreground">Runs refresh every 2 seconds, newest first. Compressed logs are retained for 7 days, with at most 1,000 completed runs, paged output with no 1 MiB capture cap, and a total compressed disk budget of 64 MiB. Older output rotates when the shared budget is full. New/existing counts are distinct flag values per target process; * marks partial historical counts.</p>
    <div class="flex flex-wrap gap-2"><Button v-if="cursors.runs" variant="outline" :disabled="busy" @click="first('runs')">Newest runs</Button><Button v-if="runs.items.length === 100 && runs.next_cursor" variant="outline" :disabled="busy" @click="next('runs')">Older runs</Button><Button variant="outline" :disabled="busy" @click="cleanup">Apply log retention policy</Button></div>
    </DisclosureSection>
    <Dialog :open="Boolean(requestedDetail)" @update:open="open=>{if(!open)closeLogs()}">
      <DialogContent class="max-h-[90vh] w-[calc(100%_-_2rem)] max-w-5xl overflow-y-auto" @close-auto-focus="restoreLogFocus">
        <DialogHeader><DialogTitle>Run logs</DialogTitle><DialogDescription>Live stdout and stderr, run details, and flag counts.</DialogDescription></DialogHeader>
        <p v-if="detailLoading&&!detail" role="status">Loading run logs…</p>
        <p v-if="error" role="alert" class="text-destructive">{{error}}</p>
        <FarmRunDetailView v-if="detail" :run="detail" :loading="detailLoading" />
        <Button variant="outline" @click="closeLogs">Close logs</Button>
      </DialogContent>
    </Dialog>
    <section v-if="versionDetail" class="space-y-2"><div class="flex items-center justify-between"><h2 class="text-lg font-semibold">{{ versionTitle }}</h2><Button variant="outline" @click="versionDetail = ''">Close versions</Button></div><pre class="max-h-96 overflow-auto rounded border p-3 text-sm">{{ versionDetail }}</pre></section>
  </div>
</template>

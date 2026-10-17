<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import type { FlagView, SubmissionView, Triage } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { date } from '@/shared/format'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import CapturedFlagForm from './CapturedFlagForm.vue'
import FlagRecords from './FlagRecords.vue'
import SubmitterForm from './SubmitterForm.vue'
const flags = ref<FlagView[]>([]), submissions = ref<SubmissionView[]>([])
const offset = ref(0), query = ref(''), search = ref(''), target = ref('')
const loading = ref(true), busy = ref(false), error = ref(''), notice = ref('')
const columns = [{ key: 'flag_id', label: 'Flag ID' }, { key: 'state', label: 'State' }, { key: 'attempts', label: 'Attempts' }, { key: 'initiated_by', label: 'Initiated by' }, { key: 'created_at', label: 'Created' }, { key: 'response', label: 'Checker outcome' }]
let mounted = true, polling = false, timer: ReturnType<typeof setInterval> | undefined
async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    const suffix = `?offset=${offset.value}&search=${encodeURIComponent(search.value)}`
    const result = await Promise.all([api<FlagView[]>(`/flags${suffix}`), api<SubmissionView[]>(`/submissions${suffix}`)])
    if (mounted && suffix === `?offset=${offset.value}&search=${encodeURIComponent(search.value)}`) [flags.value, submissions.value] = result
  } finally { loading.value = false }
}
async function action(work: () => Promise<unknown>, success = '') {
  if (busy.value) return
  busy.value = true; error.value = ''; notice.value = ''
  try { await work(); notice.value = success }
  catch (e) { error.value = errorMessage(e) }
  finally { busy.value = false }
}
function refresh() { void action(() => load()) }
function find() { search.value = query.value; offset.value = 0; refresh() }
function page(change: number) { offset.value = Math.max(0, offset.value + change); refresh() }
function submit(flag: FlagView) {
  if (flag.category !== 'captured' || flag.triage === 'own_exposed') return
  void action(async () => { await api(`/flags/${encodeURIComponent(flag.id)}/submit`, 'POST'); await load() }, 'Submission queued.')
}
function triage(flag: FlagView, resolution: string) {
  void action(async () => {
    const body: Triage = { resolution }
    if (resolution === 'captured') {
      if (!target.value.trim()) throw new Error('Enter an independently authorized enabled target ID before capture triage.')
      body.target_id = target.value.trim()
    }
    await api(`/flags/${encodeURIComponent(flag.id)}/triage`, 'POST', body)
    await load()
  }, 'Triage saved.')
}
async function poll() {
  if (!mounted || polling || busy.value) return
  polling = true
  try { await load(true) }
  catch (e) { if (mounted) error.value = errorMessage(e) }
  finally { polling = false }
}
onMounted(() => { refresh(); timer = setInterval(() => { void poll() }, 3000) })
onUnmounted(() => { mounted = false; if (timer) clearInterval(timer) })
</script>
<template>
  <div class="space-y-6">
    <header><h1 class="text-2xl font-semibold">Flag Manager</h1><p class="text-muted-foreground">Values are redacted. Only captured flags from enabled authorized targets can be submitted. Proxy findings require triage. Own exposed findings cannot be submitted.</p></header>
    <p v-if="error" role="alert" class="text-destructive">{{ error }}</p><p v-if="notice" role="status">{{ notice }}</p>
    <SubmitterForm @saved="refresh" />
    <form class="flex flex-wrap items-end gap-3" @submit.prevent="find"><div class="min-w-64 flex-1"><Label for="flag-search">Search ID, digest, category or target</Label><Input id="flag-search" v-model="query" maxlength="128" :disabled="busy" /></div><Button type="submit" :disabled="busy">Search</Button><Button type="button" variant="outline" :disabled="busy" @click="refresh">Refresh</Button></form>
    <div class="flex flex-wrap items-center gap-3"><Button variant="outline" :disabled="busy || offset === 0" @click="page(-100)">Previous</Button><Button variant="outline" :disabled="busy || (flags.length < 100 && submissions.length < 100)" @click="page(100)">Next</Button><p class="text-sm text-muted-foreground">Page {{ offset / 100 + 1 }} · {{ flags.length }} flags · {{ submissions.length }} submissions</p></div>
    <CapturedFlagForm v-model:target="target" @saved="refresh" />
    <p v-if="loading" role="status">Loading retained records…</p>
    <FlagRecords :flags="flags" :busy="busy" @submit="submit" @triage="triage" />
    <section class="space-y-3"><h2 class="text-lg font-semibold">Recent submissions</h2><DataTable :columns="columns" :rows="submissions"><template #cell-created_at="{ row }">{{ date(row.created_at) }}</template></DataTable></section>
    <p class="text-sm text-muted-foreground">Search and page through retained records. Retries preserve per-flag submission IDs; external exactly-once delivery requires checker idempotency support.</p>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import type { SubmitterConfig, SubmitterView } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import DisclosureSection from '@/components/DisclosureSection.vue'
import SubmissionBatches from './SubmissionBatches.vue'
import { pythonPackages, submitterDto } from './submitterDto'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
const emit = defineEmits<{ saved: [] }>()
const overview = ref<SubmitterView | null>(null), draft = ref<SubmitterConfig | null>(null)
const packages = ref(''), sourceMode = ref('paste'), fileName = ref(''), fileKey = ref(0)
const manualCount=ref(100), manualBusy=ref(false)
const busy = ref(false), loading = ref(true), error = ref(''), notice = ref('')
let mounted = true, polling = false, timer: ReturnType<typeof setInterval> | undefined
const example = 'import sys\n\nflags = sys.argv[1:]\n# Submit every flag to your competition checker here.\n# Exit 0 only when the whole batch has been accepted.\n'
function adopt(view: SubmitterView) {
  overview.value = view; draft.value = submitterDto(view.config)
  packages.value = (view.config.python_packages ?? []).join('\n'); fileName.value = ''; fileKey.value++
}
async function load(initial = false) {
  if (polling) return
  polling = true
  try {
    const result = await api<SubmitterView>('/submitter')
    if (!mounted) return
    if (initial) adopt(result)
    else overview.value = result // Never replace the user's unsaved configuration during polling.
  } catch (e) { if (mounted) error.value = errorMessage(e) }
  finally { polling = false; loading.value = false }
}
async function chooseFile(event: Event) {
  error.value = ''; notice.value = ''
  const input = event.target instanceof HTMLInputElement ? event.target : null
  const file = input?.files?.[0]
  if (!file || !draft.value) return
  busy.value = true
  try {
    if (!file.size || file.size > 2 * 1024 * 1024) throw new Error('Choose a Python UTF-8 file between 1 byte and 2 MiB.')
    const source = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer())
    if (mounted && draft.value) { draft.value.source = source; fileName.value = file.name }
  } catch (e) {
    error.value = e instanceof TypeError ? 'The Python file must contain valid UTF-8 text.' : errorMessage(e)
    if (input) input.value = ''
  } finally { busy.value = false }
}
async function save() {
  if (busy.value || !draft.value) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const value = submitterDto(draft.value)
    value.python_packages = pythonPackages(packages.value)
    if (!value.flag_pattern.trim()) throw new Error('Enter the flag extraction regular expression.')
    const bytes = new TextEncoder().encode(value.source).byteLength
    if (!bytes || bytes > 2 * 1024 * 1024) throw new Error('The Python source must contain 1 byte to 2 MiB of UTF-8 text.')
    value.timeout_seconds = Number(value.timeout_seconds)
    if (!Number.isInteger(value.timeout_seconds) || value.timeout_seconds < 1 || value.timeout_seconds > 120) throw new Error('Set a timeout between 1 and 120 seconds.')
    await api('/submitter', 'PUT', value)
    const result = await api<SubmitterView>('/submitter')
    if (mounted) { adopt(result); notice.value = 'Submitter configuration saved.'; emit('saved') }
  } catch (e) { if (mounted) error.value = errorMessage(e) }
  finally { busy.value = false }
}
async function runNow() {
  if(manualBusy.value)return
  manualBusy.value=true;error.value='';notice.value=''
  try {
    const count=Number(manualCount.value)
    if(!Number.isInteger(count)||count<1||count>5000)throw new Error('Choose 1 to 5000 flags.')
    await api('/submitter/run','POST',{count})
    notice.value=`Manual run requested for up to ${count} queued flags using the saved configuration.`
    await load();emit('saved')
  }catch(cause){error.value=errorMessage(cause)}
  finally{manualBusy.value=false}
}
onMounted(() => { void load(true); timer = setInterval(() => { if (!busy.value) void load() }, 3000) })
onUnmounted(() => { mounted = false; if (timer) clearInterval(timer) })
</script>
<template>
  <section class="space-y-4 rounded border p-4">
    <h2 class="text-lg font-semibold">Automatic flag submitter</h2>
    <p class="text-sm text-muted-foreground">Extract flags from Farm stdout and stderr with a regular expression into the persistent queue. Automatic submission sends new queued flags as they arrive. The Python script receives the exact captured values in sys.argv[1:], without trimming, encoding or other transformations.</p>
    <p v-if="loading" role="status">Loading submitter…</p><Button v-if="!loading && !draft" variant="outline" @click="load(true)">Retry loading submitter</Button>
    <div v-if="overview" class="flex flex-wrap gap-x-6 gap-y-2 text-sm" aria-live="polite"><p>State: <strong>{{ overview.config.enabled ? 'Enabled' : 'Paused' }}</strong></p><p>Queued: <strong>{{ overview.queued_flags }}</strong></p><p>Dispatch: {{ overview.config.enabled ? 'as flags arrive' : 'manual only' }}</p></div>
    <p v-if="overview?.runtime_available === false" role="status" class="text-sm text-muted-foreground">Python execution is unavailable. Configure the host Python runtime and uv before enabling submission.</p>
    <form v-if="draft" class="space-y-3" @submit.prevent="save">
      <div class="flex items-center gap-2"><Checkbox id="submitter-enabled" v-model="draft.enabled" :disabled="busy" /><Label for="submitter-enabled">Enable automatic submission</Label></div>
      <div><Label for="submitter-pattern">Flag extraction regex (Farm and pasted text)</Label><Input id="submitter-pattern" v-model="draft.flag_pattern" required :disabled="busy" placeholder="FLAG\{[A-Za-z0-9_]+\}" /><p class="text-sm text-muted-foreground">Matches are queued for submission. Use the exact flag format defined by your competition.</p></div>
      <div><Label for="submitter-source-mode">Python source</Label><Select id="submitter-source-mode" v-model="sourceMode" :disabled="busy"><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="paste">Paste or edit code</SelectItem><SelectItem value="file">Upload Python file</SelectItem></SelectContent></Select></div>
      <div v-if="sourceMode === 'file'"><Label for="submitter-file">UTF-8 Python file (maximum 2 MiB)</Label><Input :key="fileKey" id="submitter-file" type="file" accept=".py,text/x-python,text/plain" :disabled="busy" @change="chooseFile" /><p v-if="fileName" class="text-sm text-muted-foreground">Loaded {{ fileName }}. Review the source below, then Save.</p></div>
      <div><Label for="submitter-code">Python code</Label><Textarea id="submitter-code" v-model="draft.source" class="min-h-48 font-mono" :disabled="busy" spellcheck="false" :placeholder="example" /></div>
      <div><Label for="submitter-packages">Python dependencies (optional)</Label><Textarea id="submitter-packages" v-model="packages" :disabled="busy" placeholder="requests&#10;httpx==0.28.1" /><p class="text-sm text-muted-foreground">One package name or name==version per line, or comma separated. Compatible wheels and dependencies are prepared before execution.</p></div>
      <div class="max-w-xs"><Label for="submitter-batch-size">Flags per Python call</Label><Input id="submitter-batch-size" v-model.number="draft.batch_size" type="number" min="1" max="240" required :disabled="busy" /></div>
      <div class="max-w-xs"><Label for="submitter-timeout">Script timeout (seconds)</Label><Input id="submitter-timeout" :model-value="draft.timeout_seconds" @update:model-value="draft.timeout_seconds = Number($event)" type="number" min="1" max="120" required :disabled="busy" /></div>
      <p class="text-sm text-muted-foreground">Exit 0 accepts the whole batch. A failure keeps flags queued for retry, up to 10 attempts. Each call uses the configured count, up to 240 flags and 60 KiB; larger manual requests use multiple calls. Use checker idempotency when available to handle partial submissions safely.</p>
      <Button type="submit" :disabled="busy">{{ busy ? 'Saving…' : 'Save submitter' }}</Button>
    </form>
    <form class="flex flex-wrap items-end gap-3 border-t pt-4" @submit.prevent="runNow">
      <div><Label for="submitter-manual-count">Flags to submit now (maximum)</Label><Input id="submitter-manual-count" v-model.number="manualCount" type="number" min="1" max="5000" required :disabled="manualBusy" /></div>
      <Button type="submit" :disabled="manualBusy||!overview?.runtime_available||!overview?.config.source||['pending','running'].includes(overview?.manual_request?.status??'')">{{manualBusy?'Requesting…':'Run saved submitter now'}}</Button>
      <p class="w-full text-sm text-muted-foreground">Works while automatic submission is paused. Uses the saved code and up to this many queued flags; fewer are sent if the queue is smaller.</p>
      <p v-if="overview?.manual_request" role="status" class="w-full text-sm">Manual run: {{overview.manual_request.status}} · requested {{overview.manual_request.count}} · attempted {{overview.manual_request.flag_count}}</p>
    </form>
    <p v-if="error" role="alert" class="text-destructive">{{ error }}</p><p v-if="notice" role="status">{{ notice }}</p>
    <DisclosureSection title="Submission attempts"><SubmissionBatches /></DisclosureSection>
  </section>
</template>

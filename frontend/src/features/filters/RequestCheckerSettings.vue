<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import DisclosureSection from '@/components/DisclosureSection.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import CodeEditor from '@/components/CodeEditor.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { api, errorMessage } from '@/shared/api/client'
import type { RequestCheckerPolicy, RequestCheckerProgram, RequestCheckerPrograms, RequestCheckerSettings } from '@/generated/contracts'

type Program = RequestCheckerProgram
type Programs = RequestCheckerPrograms
const props = defineProps<{ services: Array<{ id: string; name: string }> }>()
const open = ref(false)
const settings = ref<RequestCheckerSettings>()
const programs = ref<Programs>()
const busy = ref(false)
const notice = ref('')
const error = ref('')
const showCreate = ref(false)
const newId = ref('')
const defaultSource = 'def check(request: bytes) -> bool:\n    # Return True to allow traffic, False to block it.\n    return False\n'
const newSource = ref(defaultSource)
const editing = ref<string>()
const draftSource = ref('')
const endpoint = '/filters/request-checkers'
const sourceSize = computed(() => new TextEncoder().encode(newSource.value).length)
const draftSize = computed(() => new TextEncoder().encode(draftSource.value).length)

async function load() {
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const [policyResult, programResult] = await Promise.all([
      api<RequestCheckerSettings>(endpoint),
      api<Programs>(`${endpoint}/programs`),
    ])
    settings.value = policyResult
    programs.value = programResult
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}

async function refreshPrograms() {
  programs.value = await api<Programs>(`${endpoint}/programs`)
}

async function createProgram() {
  if (!programs.value || busy.value || !/^[A-Za-z0-9_-]{1,32}$/.test(newId.value) || programs.value.programs.some(program => program.id === newId.value) || sourceSize.value < 1 || sourceSize.value > 256 * 1024) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const updated = await api<Programs>(`${endpoint}/programs/${encodeURIComponent(newId.value)}`, 'PUT', { revision: programs.value.revision, source: newSource.value })
    if (settings.value) settings.value.revision = updated.revision
    newId.value = ''; newSource.value = defaultSource; showCreate.value = false
    await load()
    notice.value = 'Checker created paused. Select its services below, then save checker settings to enable it.'
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}

function beginEdit(program: Program) { editing.value = program.id; draftSource.value = program.source }
function cancelEdit() { editing.value = undefined; draftSource.value = '' }

async function saveSource(program: Program) {
  if (!programs.value || busy.value || draftSize.value < 1 || draftSize.value > 256 * 1024) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const updated = await api<Programs>(`${endpoint}/programs/${encodeURIComponent(program.id)}`, 'PUT', { revision: programs.value.revision, source: draftSource.value })
    if (settings.value) settings.value.revision = updated.revision
    await refreshPrograms(); cancelEdit(); notice.value = `Source for ${program.id} saved.`
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}

async function chooseFile(event: Event, target: 'create' | 'edit') {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  if (!file.name.endsWith('.py') || file.size > 256 * 1024) { error.value = 'Choose a .py file no larger than 256 KiB.'; input.value = ''; return }
  const content = await file.text()
  if (new TextEncoder().encode(content).length > 256 * 1024) { error.value = 'Python source must be no larger than 256 KiB.'; input.value = ''; return }
  if (target === 'create') newSource.value = content
  else draftSource.value = content
  error.value = ''; input.value = ''
}

async function deleteProgram(program: Program) {
  if (!programs.value || busy.value || !window.confirm(`Delete checker “${program.id}”? Its /state data will be retained.`)) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const updated = await api<Programs>(`${endpoint}/programs/${encodeURIComponent(program.id)}`, 'DELETE', { revision: programs.value.revision })
    if (settings.value) settings.value.revision = updated.revision
    await load(); notice.value = `Checker ${program.id} deleted. Its /state data was retained.`
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}

async function save() {
  if (!settings.value || busy.value) return
  busy.value = true; error.value = ''; notice.value = ''
  try {
    settings.value = await api<RequestCheckerSettings>(endpoint, 'PUT', settings.value)
    if (programs.value) programs.value.revision = settings.value.revision
    notice.value = 'Request checker settings saved and applied.'
  } catch (cause) { error.value = errorMessage(cause) }
  finally { busy.value = false }
}

function options(policy: RequestCheckerPolicy) {
  const missing = policy.services.filter(id => !props.services.some(service => service.id === id))
  return [...props.services, ...missing.map(id => ({ id, name: `${id} (unavailable)` }))]
}
function usedByOther(policy: RequestCheckerPolicy, id: string) { return settings.value?.policies.some(other => other.id !== policy.id && other.services.includes(id)) }
function selectService(policy: RequestCheckerPolicy, id: string, selected: boolean) {
  policy.services = selected ? [...policy.services, id] : policy.services.filter(service => service !== id)
}

watch(open, expanded => { if (expanded && !settings.value && !busy.value) void load() })
</script>

<template>
  <DisclosureSection v-model:open="open" title="Request checkers" description="Manage Python checker programs and assign them to services.">
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    <p v-if="notice" role="status" class="text-sm">{{ notice }}</p>
    <p v-if="busy && !settings" role="status" class="text-sm text-muted-foreground">Loading checker settings…</p>

    <section class="space-y-3 rounded-md border p-3" aria-label="Python checker programs">
      <div class="flex items-center justify-between gap-3"><div><h3 class="text-sm font-medium">Python programs</h3><p class="text-xs text-muted-foreground">Paste source or upload a .py file (maximum 256 KiB).</p></div>
        <Button type="button" variant="outline" :disabled="busy" @click="showCreate = !showCreate">{{ showCreate ? 'Cancel' : 'Add Python checker' }}</Button>
      </div>
      <form v-if="showCreate" class="space-y-3 rounded border p-3" @submit.prevent="createProgram">
        <label class="block space-y-1 text-sm"><span>Checker ID</span><Input id="checker-id-input" v-model="newId" autocomplete="off" pattern="[A-Za-z0-9_-]{1,32}" maxlength="32" required /></label>
        <label class="block space-y-1 text-sm"><span>Python source</span><CodeEditor id="checker-source-input" v-model="newSource" language="python" aria-label="New Python checker source" :disabled="busy" /></label>
        <label class="block space-y-1 text-xs text-muted-foreground"><span>Upload .py file</span><input type="file" accept=".py,text/x-python" :disabled="busy" @change="chooseFile($event, 'create')" /></label>
        <p class="text-xs text-muted-foreground">{{ sourceSize }} / 262,144 bytes. True allows traffic; False blocks it. Mutable files in /state persist across source edits and checker deletion. New checkers start paused; assign services and save settings next.</p>
        <Button type="submit" :disabled="busy || !/^[A-Za-z0-9_-]{1,32}$/.test(newId) || programs?.programs.some(program => program.id === newId) || sourceSize < 1 || sourceSize > 262144">Create paused checker</Button>
      </form>
      <p v-if="programs && !programs.programs.length" class="text-sm text-muted-foreground">No checker programs are configured.</p>
      <article v-for="program in programs?.programs" :key="program.id" class="space-y-3 rounded border p-3">
        <div class="flex flex-wrap items-center justify-between gap-2"><div><h4 class="font-mono text-sm">{{ program.id }}</h4><p class="text-xs text-muted-foreground">{{ program.runtime }} · {{ program.managed ? 'UI managed' : 'Proxy configured' }}</p></div>
          <div v-if="program.managed" class="flex gap-2"><Button v-if="editing !== program.id" type="button" variant="outline" :disabled="busy" @click="beginEdit(program)">Edit source</Button><Button type="button" variant="destructive" :disabled="busy" @click="deleteProgram(program)">Delete checker</Button></div>
        </div>
        <template v-if="program.managed && editing === program.id">
          <label class="block space-y-1 text-sm"><span>Python source</span><CodeEditor id="checker-source-editor" v-model="draftSource" language="python" aria-label="Python checker source" :disabled="busy" /></label>
          <label class="block space-y-1 text-xs text-muted-foreground"><span>Upload .py file</span><input type="file" accept=".py,text/x-python" :disabled="busy" @change="chooseFile($event, 'edit')" /></label>
          <p class="text-xs text-muted-foreground">{{ draftSize }} / 262,144 bytes. Mutable files under <code>/state</code> persist across runs, source edits, and checker deletion. For example, use <code>/state/cache.json</code> for a persistent file.</p>
          <div class="flex gap-2"><Button type="button" :disabled="busy || draftSize < 1 || draftSize > 262144" @click="saveSource(program)">Save source</Button><Button type="button" variant="outline" :disabled="busy" @click="cancelEdit">Cancel</Button></div>
        </template>
        <template v-else-if="program.managed"><pre class="max-h-48 overflow-auto rounded bg-muted p-2 font-mono text-xs">{{ program.source }}</pre></template>
        <p v-else class="text-xs text-muted-foreground">This program is configured by the Proxy administrator. Its source is not available here; service assignments and policy remain editable below.</p>
      </article>
    </section>

    <p v-if="settings && !settings.policies.length" class="text-sm text-muted-foreground">Create a Python checker or configure a checker in Proxy settings.</p>
    <form v-if="settings?.policies.length" class="space-y-4" @submit.prevent="save">
      <fieldset v-for="policy in settings.policies" :key="policy.id" :disabled="busy" class="space-y-3 rounded-md border p-3">
        <legend class="px-1 font-medium">{{ policy.id }}</legend>
        <div class="flex flex-wrap items-end gap-4"><label class="space-y-1 text-sm"><span>Timeout (ms)</span><Input v-model.number="policy.timeout_ms" type="number" min="25" max="1000" required /></label>
          <div class="space-y-1 text-sm"><span>On timeout or error</span><Select :model-value="policy.allow_on_error ? 'allow' : 'block'" :disabled="busy" @update:model-value="policy.allow_on_error = $event === 'allow'"><SelectTrigger :aria-label="`Error action for ${policy.id}`"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="allow">Allow traffic</SelectItem><SelectItem value="block">Block traffic</SelectItem></SelectContent></Select></div>
        </div>
        <div class="space-y-2"><p class="text-sm font-medium">Services</p><div class="flex max-h-48 flex-wrap gap-3 overflow-y-auto"><label v-for="service in options(policy)" :key="service.id" class="flex items-center gap-2 text-sm"><Checkbox :model-value="policy.services.includes(service.id)" :disabled="busy || usedByOther(policy, service.id)" @update:model-value="selectService(policy, service.id, $event === true)" />{{ service.name }}</label></div>
          <p class="text-xs text-muted-foreground">Clear the service selection to pause this checker and keep its files.</p></div>
      </fieldset>
      <Button type="submit" :disabled="busy">Save checker settings</Button>
    </form>
    <Button type="button" variant="outline" :disabled="busy" @click="load">Reload checker settings</Button>
  </DisclosureSection>
</template>

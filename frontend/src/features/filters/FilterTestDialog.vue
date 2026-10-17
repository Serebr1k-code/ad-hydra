<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api, errorMessage } from '@/shared/api/client'
import { date } from '@/shared/format'
import { testSavedFilter } from './filterWorkflow'
import type { FilterRecord } from './FilterEditorDialog.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

interface Flow { flow_id: string; occurred_at: string; client: { ip: string; port: number } }
interface Message { message_id: string; direction: number; message_type: string; occurred_at: string; fields: Record<string, { value: unknown }> }
const props = defineProps<{ open: boolean; filter?: Pick<FilterRecord, 'id' | 'name' | 'service_id'> }>()
const emit = defineEmits<{ 'update:open': [value: boolean]; enforced: [] }>()
const isOpen = computed({ get: () => props.open, set: value => { if (!running.value) emit('update:open', value) } })
const flows = ref<Flow[]>([])
const messages = ref<Message[]>([])
const flowId = ref('')
const messageId = ref('')
const cursor = ref<number | null>(null)
const loading = ref(false)
const running = ref(false)
const error = ref('')
const result = ref('')
let generation = 0
let messageGeneration = 0
function summary(message: Message) {
  return `${message.direction === 1 ? 'Client' : 'Server'} · ${message.fields['http.method']?.value ?? message.message_type} ${message.fields['http.path']?.value ?? ''} · ${message.fields['http.user_agent']?.value ?? ''}`.slice(0, 512)
}
async function loadFlows(older = false) {
  const filter = props.filter
  if (!filter) return
  const version = ++generation
  ++messageGeneration
  loading.value = true; error.value = ''; result.value = ''
  if (!older) { flows.value = []; flowId.value = ''; messages.value = []; messageId.value = '' }
  try {
    const page = await api<{ items: Flow[]; next_cursor: number | null }>(`/traffic/flows?service_id=${encodeURIComponent(filter.service_id)}&newest_first=true&limit=25${older && cursor.value ? '&after=' + encodeURIComponent(cursor.value) : ''}`)
    if (version !== generation || !props.open) return
    flows.value = (older ? [...flows.value, ...page.items] : page.items).slice(-100)
    cursor.value = page.next_cursor
    if (!older && flows.value[0]) flowId.value = flows.value[0].flow_id
  } catch (cause) { if (version === generation) error.value = errorMessage(cause) }
  finally { if (version === generation) loading.value = false }
}
watch(() => [props.open, props.filter?.id] as const, ([open]) => {
  ++generation; ++messageGeneration
  if (open) void loadFlows()
})
watch(flowId, async id => {
  const version = ++messageGeneration
  messages.value = []; messageId.value = ''; result.value = ''; error.value = ''
  if (!id) return
  loading.value = true
  try {
    const page = await api<{ items: Message[] }>(`/traffic/flows/${encodeURIComponent(id)}/messages?limit=100`)
    if (version !== messageGeneration || !props.open) return
    messages.value = page.items
    messageId.value = (page.items.find(message => message.direction === 1) ?? page.items[0])?.message_id ?? ''
  } catch (cause) { if (version === messageGeneration) error.value = errorMessage(cause) }
  finally { if (version === messageGeneration) loading.value = false }
})
watch(messageId, () => { result.value = '' })
async function run(enforce: boolean) {
  if (!props.filter || !messageId.value || running.value) return
  const version = generation
  running.value = true; error.value = ''; result.value = ''
  try {
    const tested = await testSavedFilter(api, props.filter.id, messageId.value, enforce)
    if (version !== generation || !props.open) return
    result.value = tested.enforced ? 'Matched. Enforcement requested; check the applied revision in the list.'
      : tested.matched ? 'Matched. You can now test and enforce this rule.'
        : 'No match. Select a matching request or edit the rule. Enforcement was not changed.'
    if (tested.enforced) emit('enforced')
  } catch (cause) { if (version === generation) error.value = errorMessage(cause) }
  finally { running.value = false }
}
</script>

<template>
  <Dialog v-model:open="isOpen">
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto" @interact-outside="event => { if (running) event.preventDefault() }" @escape-key-down="event => { if (running) event.preventDefault() }">
      <DialogHeader>
        <DialogTitle>Test filter: {{ filter?.name }}</DialogTitle>
        <DialogDescription>Select captured traffic from this service. Testing evaluates saved fields without sending network traffic. Test and enforce activates the saved rule only when it matches.</DialogDescription>
      </DialogHeader>
      <div class="space-y-3">
        <p class="text-sm">If you just saved changes for enforcement, that rule is now in Shadow and observes without blocking. Complete Test and enforce to activate it.</p>
        <div class="flex gap-2"><Button variant="outline" :disabled="loading || running" @click="loadFlows()">Refresh traffic</Button><Button v-if="cursor" variant="outline" :disabled="loading || running" @click="loadFlows(true)">Older sessions</Button></div>
        <p v-if="loading" role="status">Loading captured traffic…</p>
        <p v-else-if="!flows.length" role="status">No captured sessions for this service. Send a normal request through its proxy port, then refresh traffic.</p>
        <div v-if="flows.length" class="space-y-2">
          <span class="text-sm font-medium">Session</span>
          <Select v-model="flowId" :disabled="loading || running"><SelectTrigger aria-label="Test session"><SelectValue placeholder="Select session" /></SelectTrigger><SelectContent><SelectItem v-for="flow in flows" :key="flow.flow_id" :value="flow.flow_id">{{ date(flow.occurred_at) }} · {{ flow.client.ip }}:{{ flow.client.port }} · {{ flow.flow_id.slice(0, 8) }}</SelectItem></SelectContent></Select>
        </div>
        <div v-if="messages.length" class="space-y-2">
          <span class="text-sm font-medium">Message</span>
          <Select v-model="messageId" :disabled="loading || running"><SelectTrigger aria-label="Test message"><SelectValue placeholder="Select message" /></SelectTrigger><SelectContent><SelectItem v-for="message in messages" :key="message.message_id" :value="message.message_id">{{ summary(message) }}</SelectItem></SelectContent></Select>
        </div>
        <p v-else-if="flowId && !loading" role="status">No parsed messages retained for this session. Choose another session or check its decoder profile.</p>
        <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p><p v-if="result" role="status" class="text-sm">{{ result }}</p>
      </div>
      <DialogFooter><Button variant="outline" :disabled="running" @click="isOpen = false">Close</Button><Button variant="outline" :disabled="!messageId || loading || running" @click="run(false)">Test only</Button><Button :disabled="!messageId || loading || running" @click="run(true)">Test and enforce</Button></DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { ListResponse, Page } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { openEditor } from '@/shared/editor'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
import FlowCapture from './FlowCapture.vue'
import { differences, direction, fields, messageRows, timestamp, type Message } from './traffic'
const props = defineProps<{ flowId: string }>()
const detail = ref<Record<string, unknown>>(), messages = ref<Message[]>([]), cursor = ref<number | null>()
const selected = ref<Message>(), comparison = ref<ReturnType<typeof differences>>([]), comparisonTitle = ref('')
const loading = ref(false), error = ref(''), notice = ref('')
const path = `/traffic/flows/${encodeURIComponent(props.flowId)}`
async function load(more = false) {
  loading.value = true; error.value = ''
  try {
    if (more && messages.value.length >= 500) { notice.value = 'Conversation preview is limited to 500 messages. Use the paginated API for more.'; return }
    if (!more) detail.value = await api(path)
    const page = await api<Page>(`${path}/messages${more && cursor.value ? '?after='+cursor.value : ''}`)
    messages.value = [...(more ? messages.value : []), ...messageRows(page)].slice(0,500); cursor.value = page.next_cursor
  } catch (e) { error.value = errorMessage(e) }
  finally { loading.value = false }
}
function compare(message: Message) {
  if (!selected.value) { selected.value = message; comparison.value = []; comparisonTitle.value = ''; return }
  comparisonTitle.value = `${selected.value.message_id} ↔ ${message.message_id}`
  comparison.value = differences(selected.value, message); selected.value = undefined
}
function createFilter(message: Message, field: string) {
  openEditor('Create filter from saved field', { message_id: message.message_id, field, name: 'Saved '+field, action: { type:'alert' } }, async value => {
    const filters = await api<ListResponse>('/filters')
    await api('/filters/from-message', 'POST', { ...value, expected_revision: filters.revision })
    notice.value = 'Shadow filter saved with the exact recorded value. Test it on the saved message in Filters before enforcement.'
  })
}
onMounted(() => load())
</script>
<template>
  <section class="space-y-4 border-t pt-4">
    <h2 class="text-xl font-semibold">Flow {{ flowId }}</h2>
    <Alert v-if="error" variant="destructive"><AlertDescription>{{ error }}</AlertDescription></Alert>
    <Alert v-if="notice"><AlertDescription>{{ notice }}</AlertDescription></Alert>
    <p v-if="loading" role="status">Loading conversation…</p>
    <pre v-if="detail" class="max-h-80 overflow-auto whitespace-pre-wrap rounded border p-3 text-xs">{{ JSON.stringify(detail,null,2) }}</pre>
    <FlowCapture :flow-id="flowId" />
    <h2 class="text-xl font-semibold">Bidirectional conversation</h2>
    <p v-if="selected">Selected {{ selected.message_id }}. Choose another message to compare.</p>
    <section v-if="comparisonTitle" class="space-y-2"><h3 class="font-medium">{{ comparisonTitle }}</h3><DataTable :columns="[{key:'key',label:'Field'},{key:'left',label:'Selected'},{key:'right',label:'Current'},{key:'change',label:'Difference'}]" :rows="comparison" /></section>
    <article v-for="message in messages" :key="message.message_id" class="space-y-3 rounded border p-4">
      <h3 class="font-medium">{{ timestamp(message.occurred_ms) }} · {{ direction(message.direction) }} · {{ message.message_type || message.protocol_id }}</h3>
      <p class="text-sm text-muted-foreground">Message {{ message.message_id }}</p>
      <DataTable :columns="[{key:'key',label:'Field'},{key:'type',label:'Type'},{key:'value',label:'Value'},{key:'filter',label:'Filter'}]" :rows="fields(message)"><template #cell-filter="{row}"><Button size="sm" variant="outline" @click="createFilter(message,row.key)">Create shadow filter</Button></template></DataTable>
      <Button size="sm" variant="outline" @click="compare(message)">Select / compare message</Button>
    </article>
    <p v-if="!loading && !messages.length">No decoded messages indexed. Unknown or encrypted traffic has flow metadata only.</p>
    <Button v-if="cursor" variant="outline" :disabled="loading || messages.length >= 500" @click="load(true)">More messages</Button>
    <p v-if="messages.length >= 500">Conversation preview is limited to 500 messages. Use the paginated API for more.</p>
  </section>
</template>

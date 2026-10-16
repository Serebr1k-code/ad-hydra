<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { Page } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
import TrafficQuery from './TrafficQuery.vue'
import FlowConversation from './FlowConversation.vue'
import { flowRows } from './traffic'
const rows = ref<ReturnType<typeof flowRows>>([]), cursor = ref<number | null>(), query = ref(''), selected = ref('')
const loading = ref(false), error = ref('')
let request = 0
async function load(next = false) {
  const current = ++request
  loading.value = true; error.value = ''
  const parameters = new URLSearchParams(query.value)
  if (next && cursor.value) parameters.set('after', String(cursor.value))
  try {
    const page = await api<Page>('/traffic/flows?'+parameters)
    if (current !== request) return
    rows.value = flowRows(page); cursor.value = page.next_cursor; selected.value = ''
  } catch (e) { if (current === request) error.value = errorMessage(e) }
  finally { if (current === request) loading.value = false }
}
function search(value: string) { query.value = value; void load() }
onMounted(() => load())
</script>
<template>
  <section class="space-y-4">
    <h1 class="text-2xl font-semibold">Traffic</h1>
    <TrafficQuery @search="search" />
    <p class="text-sm text-muted-foreground">Time filters use UTC. Source/destination refer to the opening client/upstream endpoints. Unknown protocols show flow metadata. Request a sample to capture the next chunk of a live flow; samples expire after 30 seconds.</p>
    <Alert v-if="error" variant="destructive"><AlertDescription>{{ error }}</AlertDescription></Alert>
    <Button variant="outline" :disabled="loading" @click="load()">{{ loading ? 'Loading…' : 'Refresh' }}</Button>
    <DataTable :columns="[{key:'service_id',label:'Service'},{key:'client',label:'Client'},{key:'started',label:'Started'},{key:'flow_id',label:'Flow'},{key:'inspect',label:'Messages'}]" :rows="rows"><template #cell-inspect="{row}"><Button size="sm" variant="outline" @click="selected = row.flow_id">Inspect</Button></template></DataTable>
    <p v-if="!loading && !rows.length">No indexed flows yet. Send traffic through a configured listener.</p>
    <Button v-if="cursor" variant="outline" :disabled="loading" @click="load(true)">Next page</Button>
    <FlowConversation v-if="selected" :key="selected" :flow-id="selected" />
  </section>
</template>

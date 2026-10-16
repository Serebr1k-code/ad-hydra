<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Capture, Sample } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
const props = defineProps<{ flowId: string }>()
const capture = ref<Capture>(), sample = ref<Sample>(), busy = ref(false), error = ref('')
const preview = computed(() => sample.value?.data.slice(0,4096) ?? [])
const hex = computed(() => preview.value.map(byte => byte.toString(16).padStart(2,'0')).join(' '))
const ascii = computed(() => preview.value.map(byte => byte >= 32 && byte < 127 ? String.fromCharCode(byte) : '.').join(''))
async function request() {
  busy.value = true; error.value = ''; sample.value = undefined; capture.value = undefined
  try { capture.value = await api<Capture>(`/traffic/flows/${encodeURIComponent(props.flowId)}/capture`, 'POST') }
  catch (e) { error.value = errorMessage(e) }
  finally { busy.value = false }
}
async function read() {
  if (!capture.value) return
  busy.value = true; error.value = ''
  try { sample.value = await api<Sample>(`/traffic/samples/${encodeURIComponent(capture.value.sample_id)}`) }
  catch (e) {
    error.value = e !== null && typeof e === 'object' && 'status' in e && e.status === 410
      ? `Sample is waiting for traffic, expired, or unavailable. Request another sample on a live flow. ${errorMessage(e)}`
      : errorMessage(e)
  } finally { busy.value = false }
}
</script>
<template>
  <section class="space-y-3">
    <Button variant="outline" :disabled="busy" @click="request">{{ busy ? 'Working…' : 'Capture next chunk' }}</Button>
    <Alert v-if="error" variant="destructive"><AlertDescription>{{ error }}</AlertDescription></Alert>
    <template v-if="capture && !sample"><p>Sample {{ capture.sample_id }} requested. Send traffic, then read the sample.</p><Button :disabled="busy" @click="read">Read sample</Button></template>
    <template v-if="sample"><p>Expires {{ sample.expires_at }}{{ sample.truncated ? ' · captured chunk truncated' : '' }}</p><h3 class="font-medium">HEX</h3><pre class="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded border p-3">{{ hex || '(empty)' }}</pre><h3 class="font-medium">ASCII</h3><pre class="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded border p-3">{{ ascii || '(empty)' }}</pre><p v-if="sample.data.length > 4096">Preview shows the first 4096 bytes.</p></template>
  </section>
</template>

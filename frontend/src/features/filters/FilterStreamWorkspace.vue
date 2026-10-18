<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { events, type LiveState } from '@/shared/api/client'
import { parseEventFrame } from '@/shared/api/sse'
import FlowConversation from '@/features/traffic/FlowConversation.vue'
import { trafficEvent } from '@/features/traffic/live'

export interface StreamPreviewResult {
  flow_id: string
  state: string
  matched: boolean
  would_block: boolean
  matched_messages: number
}

const props = defineProps<{
  flowIds: string[]
  protocolId: string
  results: StreamPreviewResult[]
  busy: boolean
  error: string
  enabled: boolean
  mode: string
}>()
const selected = ref('')
const conversation = ref<InstanceType<typeof FlowConversation>>()
const liveState = ref<LiveState>('connecting')
const resultsById = computed(() => new Map(props.results.map(result => [result.flow_id, result])))
const controller = new AbortController()
watch(() => props.flowIds, ids => {
  if (!ids.includes(selected.value)) selected.value = ids[0] ?? ''
}, { immediate: true })
function status(id: string): string {
  if (!props.enabled) return 'Filter disabled'
  if (props.busy) return 'Updating preview…'
  const result = resultsById.value.get(id)
  if (!result) return props.error ? 'Preview unavailable' : 'Waiting for preview'
  if (result.state === 'active') return 'Live · preview after closure'
  if (result.state === 'not_retained') return 'History unavailable'
  if (result.state === 'decoder_error') return 'Decoder could not replay stream'
  const partial = result.state === 'partial' ? ' · partial capture' : ''
  if (result.would_block) return 'Would block' + partial
  if (result.matched) return (props.mode === 'shadow' ? 'Matches · observe only' : 'Matches · non-blocking action') + partial
  return result.state === 'partial' ? 'No match in partial capture' : 'No match · would pass'
}
onMounted(() => {
  void events(frame => {
    try {
      if (parseEventFrame(frame)?.event === 'loss') { conversation.value?.resync(); return }
      const event = trafficEvent(frame)
      if (event?.kind === 'event_loss') conversation.value?.resync()
      else if (event?.flow_id === selected.value) conversation.value?.receive(event)
    } catch { conversation.value?.resync() }
  }, controller.signal, state => {
    liveState.value = state
    if (state === 'live') conversation.value?.resync()
  })
})
onUnmounted(() => controller.abort())
</script>

<template>
  <section id="filter-stream-workspace" aria-label="Selected streams" class="flex min-h-[480px] min-w-0 flex-col overflow-hidden rounded-md border lg:min-h-0">
    <header class="shrink-0 space-y-2 border-b p-3">
      <div class="flex items-center justify-between gap-2"><h3 class="font-medium">Selected streams ({{ flowIds.length }})</h3><span class="text-xs text-muted-foreground">{{ liveState === 'live' ? 'Live' : 'Connecting…' }}</span></div>
      <p class="text-xs text-muted-foreground">Click a stream to inspect it while editing the filter.</p>
      <div class="max-h-40 space-y-1 overflow-y-auto">
        <Button v-for="id in flowIds" :key="id" type="button" variant="ghost" :data-preview-flow-id="id" :aria-pressed="selected === id" :aria-label="'Open stream ' + id" class="h-auto w-full justify-start whitespace-normal px-2 py-2 text-left" :class="selected === id ? 'bg-accent' : ''" @click="selected = id">
          <span class="min-w-0"><span class="block break-all font-mono text-xs">{{ id }}</span><span class="block text-xs" :class="resultsById.get(id)?.would_block && !busy ? 'text-destructive' : 'text-muted-foreground'">{{ status(id) }}</span></span>
        </Button>
      </div>
      <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    </header>
    <FlowConversation v-if="selected" :key="selected + ':' + protocolId" ref="conversation" class="min-h-0 flex-1" :flow-id="selected" :protocol-id="protocolId" :show-blocked="true" />
  </section>
</template>

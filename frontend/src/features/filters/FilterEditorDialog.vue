<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import type { FilterInput } from '@/generated/contracts'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import RuleGroupEditor from './RuleGroupEditor.vue'
import { createGroup, fromExpression, toExpression, type Group } from './ruleBuilder'
import { fieldsForService, type DecoderCatalog } from './fieldCatalog'
import type { RuleField } from './ruleBuilder'
import { api, errorMessage } from '@/shared/api/client'
import FilterStreamWorkspace, { type StreamPreviewResult } from './FilterStreamWorkspace.vue'
import { serviceProtocol } from '@/features/traffic/traffic'

export interface FilterRecord extends Omit<FilterInput, 'expected_revision'> {
  id: string
  created_at: string
  created_by: string
  hit_count: number
}
interface ServiceOption { id: string; name: string; transport: string; protocol_id: string }
const emptyCatalog: DecoderCatalog = { items: [], activations: [], native_schemas: [] }
const props = defineProps<{ open: boolean; busy: boolean; error: string; services: ServiceOption[]; catalog?: DecoderCatalog; filter?: FilterRecord; initialServiceId?: string; initialFlowIds?: string[] }>()
const emit = defineEmits<{ 'update:open': [value: boolean]; save: [value: Omit<FilterInput, 'expected_revision'>] }>()
const isOpen = computed({ get: () => props.open, set: value => emit('update:open', value) })
const hasSelectedStreams = computed(() => Boolean(props.initialFlowIds?.length))
const streamProtocol = computed(() => {
  const service = props.services.find(item => item.id === props.initialServiceId)
  return service ? serviceProtocol(service, props.catalog?.activations ?? []) : 'raw'
})
const selectedServices = computed(() => props.services.filter(service =>
  service.id === form.service_id || form.additional_service_ids.includes(service.id),
))
const availableFields = computed<RuleField[]>(() => {
  const catalogs = selectedServices.value.map(service => fieldsForService(service, props.catalog ?? emptyCatalog))
  const [first, ...rest] = catalogs
  return (first ?? []).filter(field => rest.every(catalog =>
    catalog.some(other => other.value === field.value && other.kind === field.kind && other.schemaType === field.schemaType),
  ))
})
const supportsTcp = computed(() => selectedServices.value.every(service => service.transport === 'tcp'))
const supportsUdp = computed(() => selectedServices.value.every(service => service.transport === 'udp'))
const supportsHttp = computed(() => selectedServices.value.length > 0 && selectedServices.value.every(service =>
  service.transport === 'tcp' && serviceProtocol(service, props.catalog?.activations ?? []) === 'http',
))
const supportsChosenAction = computed(() => {
  switch (form.actionType) {
    case 'block': return supportsHttp.value
    case 'drop_connection': return supportsTcp.value
    case 'drop_datagram': return supportsUdp.value
    default: return true
  }
})
function changeService(id: string, selected: boolean) {
  const ids = form.additional_service_ids.filter(other => other !== id)
  form.additional_service_ids = selected ? [...ids, id] : ids
}
function selectAllServices() {
  form.additional_service_ids = props.services.filter(service => service.id !== form.service_id).map(service => service.id)
}
const defaultField = computed(() => availableFields.value.find(field => !['body.size','transfer.bytes','packet.size','body.size_scope','network.src_ip','network.src_port','network.dst_ip','network.dst_port','transport.kind','direction'].includes(field.value))?.value ?? availableFields.value[0]?.value ?? 'http.path')
const ruleError = ref('')
const expressionEditable = ref(true)
const preservedAction = ref<Record<string, unknown> | null>(null)
const form = reactive({
  service_id: '', additional_service_ids: [] as string[], name: '', description: '', enabled: true, mode: 'enforce', priority: 10,
  actionType: 'block', httpStatus: 403, tag: '', expires: '', rules: createGroup(),
})
const previewRows = ref<StreamPreviewResult[]>([])
const previewBusy = ref(false)
const previewError = ref('')
let previewTimer: ReturnType<typeof setTimeout> | undefined
let previewGeneration = 0
function schedulePreview() {
  if (previewTimer) clearTimeout(previewTimer)
  const flowIds = props.initialFlowIds ?? []
  const generation = ++previewGeneration
  previewRows.value = []
  previewError.value = ''
  if (!props.open || !flowIds.length || !form.service_id) { previewBusy.value = false; return }
  previewBusy.value = true
  previewTimer = setTimeout(async () => {
    try {
      if (form.additional_service_ids.length) ensureSharedFields(form.rules)
      const expression = toExpression(form.rules, availableFields.value)
      const action: Record<string, unknown> = form.actionType === '__preserve__' && preservedAction.value ? preservedAction.value : form.actionType === 'block'
        ? { type: 'block', http_status: form.httpStatus }
        : form.actionType === 'tag' ? { type: 'tag', tag: form.tag } : { type: form.actionType }
      const { additional_service_ids: _additional, ...payload } = buildPayload({ expression, action })
      const response = await api<{ results: StreamPreviewResult[] }>('/filters/preview', 'POST', { ...payload, flow_ids: flowIds })
      if (generation === previewGeneration) previewRows.value = response.results
    } catch (cause) {
      if (generation === previewGeneration) previewError.value = errorMessage(cause)
    } finally {
      if (generation === previewGeneration) previewBusy.value = false
    }
  }, 250)
}
watch(() => [props.open, form.service_id, form.additional_service_ids, form.name, form.description, form.enabled, form.mode, form.priority, form.actionType, form.httpStatus, form.tag, form.expires, form.rules, props.initialFlowIds] as const, schedulePreview, { deep: true })
onBeforeUnmount(() => { if (previewTimer) clearTimeout(previewTimer); previewGeneration++ })
watch(() => [props.open, props.filter, props.services] as const, ([open]) => {
  if (!open) return
  const filter = props.filter
  const action = filter?.action ?? { type: props.services[0]?.transport === 'udp' ? 'drop_datagram' : 'drop_connection' }
  const supportedActions = ['block', 'alert', 'log', 'allow', 'drop_connection', 'drop_datagram', 'tag']
  const actionType = typeof action.type === 'string' && supportedActions.includes(action.type) ? action.type : filter ? '__preserve__' : 'block'
  preservedAction.value = actionType === '__preserve__' ? structuredClone(action) : null
  ruleError.value = ''
  expressionEditable.value = true
  form.service_id = filter?.service_id ?? props.initialServiceId ?? props.services[0]?.id ?? ''
  form.additional_service_ids = [...(filter?.additional_service_ids ?? [])]
  form.name = filter?.name ?? 'New filter'
  form.description = filter?.description ?? ''
  form.enabled = filter?.enabled ?? true
  form.mode = filter?.mode ?? 'enforce'
  form.priority = filter?.priority ?? 10
  form.actionType = actionType
  form.httpStatus = typeof action.http_status === 'number' ? action.http_status : 403
  form.tag = typeof action.tag === 'string' ? action.tag : ''
  const localExpiration = filter?.expires_at_ms ? new Date(filter.expires_at_ms - new Date().getTimezoneOffset() * 60_000) : null
  form.expires = localExpiration ? localExpiration.toISOString().slice(0, 16) : ''
  try { form.rules = filter ? fromExpression(filter.expression) : createGroup('all', defaultField.value, availableFields.value) }
  catch {
    form.rules = createGroup()
    expressionEditable.value = false
    ruleError.value = 'This saved expression is outside the visual builder format; saving is disabled to preserve it.'
  }
})
function ensureSharedFields(node: import('./ruleBuilder').RuleNode) {
  if (node.kind === 'group') {
    for (const child of node.children) ensureSharedFields(child)
    return
  }
  if (!availableFields.value.some(field => field.value === node.field)) {
    throw new Error(`Field ${node.field} is not available on every selected service.`)
  }
}

function save() {
  if (!expressionEditable.value) return
  if (!supportsChosenAction.value) {
    ruleError.value = 'Choose an action supported by every selected service.'
    return
  }
  let expression: Record<string, unknown>
  try {
    if (form.additional_service_ids.length) ensureSharedFields(form.rules)
    expression = toExpression(form.rules, availableFields.value)
    ruleError.value = ''
  }
  catch (cause) { ruleError.value = cause instanceof Error ? cause.message : 'Invalid rules'; return }
  const action: Record<string, unknown> = form.actionType === '__preserve__' && preservedAction.value ? preservedAction.value : form.actionType === 'block'
    ? { type: 'block', http_status: form.httpStatus }
    : form.actionType === 'tag' ? { type: 'tag', tag: form.tag }
      : { type: form.actionType }
  emit('save', buildPayload({ expression, action }))
}
watch(() => form.service_id, id => {
  if (form.additional_service_ids.includes(id)) changeService(id, false)
  if (!props.filter && availableFields.value.length) form.rules = createGroup('all', defaultField.value, availableFields.value)
  if (!props.filter && ['drop_connection', 'drop_datagram'].includes(form.actionType)) {
    form.actionType = props.services.find(service => service.id === id)?.transport === 'udp' ? 'drop_datagram' : 'drop_connection'
  }
})
function buildPayload(overrides: Record<string, unknown>): Omit<FilterInput, 'expected_revision'> {
  const expires = form.expires ? new Date(form.expires).getTime() : null
  return {
    service_id: form.service_id, additional_service_ids: form.additional_service_ids, name: form.name, description: form.description,
    enabled: form.enabled, mode: form.mode, priority: form.priority,
    expression: overrides.expression as Record<string, unknown> ?? toExpression(form.rules, availableFields.value),
    action: overrides.action as Record<string, unknown> ?? { type: 'block', http_status: form.httpStatus },
    backend: 'userspace', expires_at_ms: Number.isFinite(expires) ? expires : null,
  }
}
</script>

<template>
  <Dialog v-model:open="isOpen">
    <DialogContent :class="hasSelectedStreams ? 'flex h-[94dvh] w-[96vw] max-w-[1800px] flex-col overflow-hidden' : 'max-h-[90vh] max-w-4xl overflow-y-auto'">
      <DialogHeader>
        <DialogTitle>{{ filter ? 'Edit filter' : 'Create filter' }}</DialogTitle>
        <DialogDescription>New filters enforce when saved and applied by the proxy. Choose Shadow to observe matches without applying the action; traffic testing is optional.</DialogDescription>
      </DialogHeader>
      <div :class="hasSelectedStreams ? 'grid min-h-0 flex-1 gap-4 overflow-y-auto lg:grid-cols-2 lg:overflow-hidden' : ''">
      <form id="filter-editor-form" class="min-w-0 space-y-5" :class="hasSelectedStreams ? 'lg:overflow-y-auto lg:pr-3' : ''" @submit.prevent="save">
        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2"><Label for="filter-name">Name</Label><Input id="filter-name" v-model="form.name" required maxlength="256" /></div>
          <div class="space-y-2"><Label>Service</Label><Select v-model="form.service_id" :disabled="hasSelectedStreams"><SelectTrigger aria-label="Filter service"><SelectValue placeholder="Choose service" /></SelectTrigger><SelectContent><SelectItem v-for="service in services" :key="service.id" :value="service.id">{{ service.name }}</SelectItem></SelectContent></Select></div>
          <fieldset v-if="services.length > 1" class="space-y-2 md:col-span-2">
            <legend class="text-sm font-medium">Also apply to</legend>
            <div class="flex gap-2">
              <Button type="button" variant="outline" size="sm" @click="selectAllServices">Select all services</Button>
              <Button v-if="form.additional_service_ids.length" type="button" variant="ghost" size="sm" @click="form.additional_service_ids = []">Clear additional services</Button>
            </div>
            <div class="max-h-36 space-y-2 overflow-y-auto">
              <template v-for="service in services" :key="service.id">
                <label v-if="service.id !== form.service_id" class="flex items-center gap-2 text-sm">
                  <Checkbox :model-value="form.additional_service_ids.includes(service.id)" @update:model-value="changeService(service.id, $event === true)" />
                  {{ service.name }}
                </label>
              </template>
            </div>
            <p class="text-xs text-muted-foreground">The rule editor shows fields available on every selected service.</p>
          </fieldset>
          <div class="space-y-2"><Label for="filter-mode">Mode</Label><Select v-model="form.mode"><SelectTrigger id="filter-mode"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="shadow">Shadow (observe only)</SelectItem><SelectItem value="enforce">Enforce</SelectItem></SelectContent></Select></div>
          <div class="space-y-2"><Label for="filter-priority">Priority</Label><Input id="filter-priority" v-model.number="form.priority" type="number" min="0" max="1000000" /></div>
          <div class="space-y-2 md:col-span-2"><Label for="filter-description">Description</Label><Textarea id="filter-description" v-model="form.description" maxlength="4096" rows="2" /></div>
        </div>

        <p v-if="form.mode === 'shadow'" role="status" class="rounded-md border p-3 text-sm">Shadow mode: matches are counted, but traffic is allowed through. Choose Enforce to apply the action immediately; saved-traffic testing is optional.</p>

        <p v-if="form.mode === 'enforce'" role="status" class="rounded-md border p-3 text-sm">Saving applies this rule as soon as the proxy acknowledges the new revision. No traffic test is required.</p>

        <section class="space-y-2" :aria-disabled="!expressionEditable">
          <div><h3 class="font-medium">Match conditions</h3><p class="text-sm text-muted-foreground">Groups add parentheses. Choose whether all or any conditions inside a group must match. Each condition has its own direction. Conditions evaluate one message at a time; use OR to match either a client-only or a server-only condition.</p></div>
          <RuleGroupEditor :group="form.rules" :fields="availableFields" />
          <p class="text-xs text-muted-foreground">Live byte limits count bytes as they arrive. HTTP counts body bytes per request; other TCP profiles count each stream direction, and UDP counts each datagram. Use Block transfer to stop the selected direction immediately; on TCP this closes the whole connection. Block HTTP request is for client requests.</p>
        </section>

        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2"><Label>Action</Label><Select v-model="form.actionType"><SelectTrigger aria-label="Filter action"><SelectValue /></SelectTrigger><SelectContent><SelectItem v-if="!supportsChosenAction" :value="form.actionType" disabled>Action unavailable for selected services</SelectItem><SelectItem v-if="form.actionType === '__preserve__'" value="__preserve__">Keep existing advanced action</SelectItem><SelectItem v-if="supportsHttp" value="block">Block HTTP request</SelectItem><SelectItem value="alert">Alert</SelectItem><SelectItem value="log">Log</SelectItem><SelectItem value="allow">Allow</SelectItem><SelectItem v-if="supportsTcp" value="drop_connection">Block transfer (close TCP connection)</SelectItem><SelectItem v-if="supportsUdp" value="drop_datagram">Block transfer (drop UDP datagram)</SelectItem><SelectItem value="tag">Tag</SelectItem></SelectContent></Select></div>
          <div v-if="form.actionType === 'block'" class="space-y-2"><Label for="filter-status">HTTP block status</Label><Input id="filter-status" v-model.number="form.httpStatus" type="number" min="400" max="599" /></div>
          <div v-else-if="form.actionType === 'tag'" class="space-y-2"><Label for="filter-tag">Tag</Label><Input id="filter-tag" v-model="form.tag" required maxlength="128" /></div>
          <div class="space-y-2"><Label for="filter-expires">Expires (optional)</Label><Input id="filter-expires" v-model="form.expires" type="datetime-local" /></div>
          <label class="flex items-center gap-2 self-end pb-2"><Checkbox :model-value="form.enabled" @update:model-value="value => form.enabled = value === true" /><span class="text-sm">Enabled</span></label>
        </div>
        <p v-if="ruleError || error" role="alert" class="text-sm text-destructive">{{ ruleError || error }}</p>
        <DialogFooter><Button type="button" variant="outline" :disabled="busy" @click="isOpen = false">Cancel</Button><Button type="submit" :disabled="busy || !form.service_id || !expressionEditable">{{ busy ? 'Saving…' : 'Save filter' }}</Button></DialogFooter>
      </form>
      <FilterStreamWorkspace v-if="open && initialFlowIds?.length" :flow-ids="initialFlowIds" :protocol-id="streamProtocol" :results="previewRows" :busy="previewBusy" :error="previewError" :enabled="form.enabled" :mode="form.mode" />
      </div>
    </DialogContent>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
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

export interface FilterRecord extends Omit<FilterInput, 'expected_revision'> {
  id: string
  created_at: string
  created_by: string
  hit_count: number
}
interface ServiceOption { id: string; name: string; transport: string }
const props = defineProps<{ open: boolean; busy: boolean; error: string; services: ServiceOption[]; filter?: FilterRecord }>()
const emit = defineEmits<{ 'update:open': [value: boolean]; save: [value: Omit<FilterInput, 'expected_revision'>] }>()
const isOpen = computed({ get: () => props.open, set: value => emit('update:open', value) })
const ruleError = ref('')
const expressionEditable = ref(true)
const preservedAction = ref<Record<string, unknown> | null>(null)
const form = reactive({
  service_id: '', name: '', description: '', enabled: true, mode: 'enforce', priority: 10,
  actionType: 'block', httpStatus: 403, tag: '', expires: '', rules: createGroup(),
})
watch(() => [props.open, props.filter, props.services] as const, ([open]) => {
  if (!open) return
  const filter = props.filter
  const action = filter?.action ?? { type: props.services[0]?.transport === 'udp' ? 'drop_datagram' : 'drop_connection' }
  const supportedActions = ['block', 'alert', 'log', 'allow', 'drop_connection', 'drop_datagram', 'tag']
  const actionType = typeof action.type === 'string' && supportedActions.includes(action.type) ? action.type : filter ? '__preserve__' : 'block'
  preservedAction.value = actionType === '__preserve__' ? structuredClone(action) : null
  ruleError.value = ''
  expressionEditable.value = true
  form.service_id = filter?.service_id ?? props.services[0]?.id ?? ''
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
  try { form.rules = filter ? fromExpression(filter.expression) : createGroup() }
  catch {
    form.rules = createGroup()
    expressionEditable.value = false
    ruleError.value = 'This saved expression is outside the visual builder format; saving is disabled to preserve it.'
  }
})
function save() {
  if (!expressionEditable.value) return
  let expression: Record<string, unknown>
  try { expression = toExpression(form.rules); ruleError.value = '' }
  catch (cause) { ruleError.value = cause instanceof Error ? cause.message : 'Invalid rules'; return }
  const action: Record<string, unknown> = form.actionType === '__preserve__' && preservedAction.value ? preservedAction.value : form.actionType === 'block'
    ? { type: 'block', http_status: form.httpStatus }
    : form.actionType === 'tag' ? { type: 'tag', tag: form.tag }
      : { type: form.actionType }
  emit('save', buildPayload({ expression, action }))
}
watch(() => form.service_id, id => {
  if (!props.filter && ['drop_connection', 'drop_datagram'].includes(form.actionType)) {
    form.actionType = props.services.find(service => service.id === id)?.transport === 'udp' ? 'drop_datagram' : 'drop_connection'
  }
})
function buildPayload(overrides: Record<string, unknown>): Omit<FilterInput, 'expected_revision'> {
  const expires = form.expires ? new Date(form.expires).getTime() : null
  return {
    service_id: form.service_id, name: form.name, description: form.description,
    enabled: form.enabled, mode: form.mode, priority: form.priority,
    expression: overrides.expression as Record<string, unknown> ?? toExpression(form.rules),
    action: overrides.action as Record<string, unknown> ?? { type: 'block', http_status: form.httpStatus },
    backend: 'userspace', expires_at_ms: Number.isFinite(expires) ? expires : null,
  }
}
</script>

<template>
  <Dialog v-model:open="isOpen">
    <DialogContent class="max-h-[90vh] max-w-4xl overflow-y-auto">
      <DialogHeader>
        <DialogTitle>{{ filter ? 'Edit filter' : 'Create filter' }}</DialogTitle>
        <DialogDescription>New filters enforce when saved and applied by the proxy. Choose Shadow to observe matches without applying the action; traffic testing is optional.</DialogDescription>
      </DialogHeader>
      <form class="space-y-5" @submit.prevent="save">
        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2"><Label for="filter-name">Name</Label><Input id="filter-name" v-model="form.name" required maxlength="256" /></div>
          <div class="space-y-2"><Label>Service</Label><Select v-model="form.service_id"><SelectTrigger aria-label="Filter service"><SelectValue placeholder="Choose service" /></SelectTrigger><SelectContent><SelectItem v-for="service in services" :key="service.id" :value="service.id">{{ service.name }}</SelectItem></SelectContent></Select></div>
          <div class="space-y-2"><Label for="filter-mode">Mode</Label><Select v-model="form.mode"><SelectTrigger id="filter-mode"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="shadow">Shadow (observe only)</SelectItem><SelectItem value="enforce">Enforce</SelectItem></SelectContent></Select></div>
          <div class="space-y-2"><Label for="filter-priority">Priority</Label><Input id="filter-priority" v-model.number="form.priority" type="number" min="0" max="1000000" /></div>
          <div class="space-y-2 md:col-span-2"><Label for="filter-description">Description</Label><Textarea id="filter-description" v-model="form.description" maxlength="4096" rows="2" /></div>
        </div>

        <p v-if="form.mode === 'shadow'" role="status" class="rounded-md border p-3 text-sm">Shadow mode: matches are counted, but traffic is allowed through. Choose Enforce to apply the action immediately; saved-traffic testing is optional.</p>

        <p v-if="form.mode === 'enforce'" role="status" class="rounded-md border p-3 text-sm">Saving applies this rule as soon as the proxy acknowledges the new revision. No traffic test is required.</p>

        <section class="space-y-2" :aria-disabled="!expressionEditable">
          <div><h3 class="font-medium">Match conditions</h3><p class="text-sm text-muted-foreground">Groups add parentheses. Choose whether all or any conditions inside a group must match.</p></div>
          <RuleGroupEditor :group="form.rules" />
          <p class="text-xs text-muted-foreground">Live byte limits count bytes as they arrive. HTTP counts body bytes per request; other TCP profiles count each stream direction, and UDP counts each datagram. Use Block transfer to stop the upload immediately.</p>
        </section>

        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2"><Label>Action</Label><Select v-model="form.actionType"><SelectTrigger aria-label="Filter action"><SelectValue /></SelectTrigger><SelectContent><SelectItem v-if="form.actionType === '__preserve__'" value="__preserve__">Keep existing advanced action</SelectItem><SelectItem value="block">Block HTTP request</SelectItem><SelectItem value="alert">Alert</SelectItem><SelectItem value="log">Log</SelectItem><SelectItem value="allow">Allow</SelectItem><SelectItem value="drop_connection">Block transfer (close TCP connection)</SelectItem><SelectItem value="drop_datagram">Block transfer (drop UDP datagram)</SelectItem><SelectItem value="tag">Tag</SelectItem></SelectContent></Select></div>
          <div v-if="form.actionType === 'block'" class="space-y-2"><Label for="filter-status">HTTP block status</Label><Input id="filter-status" v-model.number="form.httpStatus" type="number" min="400" max="599" /></div>
          <div v-else-if="form.actionType === 'tag'" class="space-y-2"><Label for="filter-tag">Tag</Label><Input id="filter-tag" v-model="form.tag" required maxlength="128" /></div>
          <div class="space-y-2"><Label for="filter-expires">Expires (optional)</Label><Input id="filter-expires" v-model="form.expires" type="datetime-local" /></div>
          <label class="flex items-center gap-2 self-end pb-2"><Checkbox :model-value="form.enabled" @update:model-value="value => form.enabled = value === true" /><span class="text-sm">Enabled</span></label>
        </div>
        <p v-if="ruleError || error" role="alert" class="text-sm text-destructive">{{ ruleError || error }}</p>
        <DialogFooter><Button type="button" variant="outline" :disabled="busy" @click="isOpen = false">Cancel</Button><Button type="submit" :disabled="busy || !form.service_id || !expressionEditable">{{ busy ? 'Saving…' : 'Save filter' }}</Button></DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>

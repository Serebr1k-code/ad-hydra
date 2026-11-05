<script setup lang="ts">
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { recommendedProtection, type ServiceProtection } from './protection'
const model = defineModel<ServiceProtection | null>({ required: true })
defineProps<{ disabled: boolean; transport: string }>()
const fields: {key:keyof ServiceProtection;label:string;min:number}[] = [
  {key:'max_active_flows',label:'Maximum active TCP connections / UDP sessions',min:1},
  {key:'tcp_connections_per_second',label:'New TCP connections per second',min:0},
  {key:'udp_datagrams_per_second',label:'UDP datagrams per second',min:0},
  {key:'bytes_per_second',label:'Bytes per second (both directions combined)',min:0},
  {key:'idle_timeout_ms',label:'Idle timeout (milliseconds)',min:1},
]
</script>
<template>
  <div class="space-y-3 rounded-md border p-3">
    <div class="flex items-center gap-2"><Checkbox id="service-protection" :model-value="model !== null" :disabled="disabled" @update:model-value="value => model = value === true ? recommendedProtection() : null" /><Label for="service-protection">Enable service overload protection</Label></div>
    <p class="text-sm text-muted-foreground">Limits cover all clients collectively, including clients sharing an IP. Disabling protection keeps existing relay behaviour.</p>
    <template v-if="model">
      <Button type="button" size="sm" variant="outline" :disabled="disabled" @click="model = recommendedProtection()">Use recommended limits</Button>
      <div v-for="field in fields" :key="field.key" class="space-y-1">
        <Label :for="'protection-' + field.key">{{ field.label }}</Label>
        <Input :id="'protection-' + field.key" v-model.number="model[field.key]" type="number" :min="field.min" step="1" required :disabled="disabled" />
      </div>
      <p class="text-sm text-muted-foreground">Zero disables a rate limit. Burst allowance is one second. TCP admission or byte overflow closes the entire connection; UDP overflow drops the entire datagram. UDP uses the datagram rate; TCP uses the connection rate. Idle connections or UDP sessions expire after the timeout.</p>
      <p class="text-sm text-muted-foreground">{{ transport.toUpperCase() }} limits do not protect against kernel SYN floods or a saturated network link.</p>
    </template>
  </div>
</template>

<script setup lang="ts">
import type { EditableConfig } from '@/generated/contracts'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Checkbox } from '@/components/ui/checkbox'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import CheckerSettings from './CheckerSettings.vue'
const model = defineModel<EditableConfig>({ required: true })
function execution(value: unknown) {
  model.value.execution_mode = value === 'host' ? 'host' : 'sandbox'
  if (value === 'host' && !model.value.host_runtime) model.value.host_runtime = { uv: '/usr/local/bin/uv', work_dir: '/var/lib/hydra/host-runs' }
}
function sandbox(value: boolean | 'indeterminate') {
  model.value.sandbox = value ? { delegated_cgroup: '/sys/fs/cgroup/hydra', runtime_dir: '/run/hydra/workers', bwrap: '/usr/bin/bwrap', ip: '/usr/sbin/ip', uv: '/usr/local/bin/uv' } : null
}
</script>
<template>
  <div class="space-y-4">
    <fieldset class="space-y-2 rounded-md border p-4"><legend class="px-1 font-medium">Traffic history</legend><Label for="settings-history-limit">Saved traffic per service (MiB)</Label><Input id="settings-history-limit" type="number" min="1" max="1024" step="1" required :model-value="model.traffic_history_per_service_mib??128" @update:model-value="model.traffic_history_per_service_mib=Number($event)" /><p class="text-sm text-muted-foreground">Default 128 MiB for each service, including gzip traffic and metadata. Oldest streams rotate within their own service. Applies live; disconnected Proxy receives the setting when it reconnects. Completed streams retain up to 64 KiB per direction for 24 hours.</p></fieldset>

    <div><Label for="settings-pattern">Flag regex</Label><Input id="settings-pattern" :model-value="model.flag_pattern || ''" @update:model-value="model.flag_pattern = String($event) || null" maxlength="4096" /></div>
    <div class="flex items-center gap-2"><Checkbox id="settings-auto-submit" :model-value="model.auto_submit_captured" @update:model-value="model.auto_submit_captured = $event === true" /><Label for="settings-auto-submit">Automatically submit captured flags</Label></div>
    <CheckerSettings v-model="model.checker" />
    <fieldset class="space-y-3 rounded-md border p-4">
      <legend class="px-1 font-medium">Exploit execution</legend>
      <Label for="settings-execution">Execution mode</Label><Select :model-value="model.execution_mode" @update:model-value="execution"><SelectTrigger id="settings-execution"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="sandbox">Sandbox</SelectItem><SelectItem value="host">Host child processes</SelectItem></SelectContent></Select>
      <p v-if="model.execution_mode === 'host'" class="text-sm text-muted-foreground">Uploaded code runs with Core's host permissions. Use trusted competition code.</p>
      <template v-if="model.host_runtime">
        <Label for="settings-host-uv">Host uv executable</Label><Input id="settings-host-uv" v-model="model.host_runtime.uv" required />
        <Label for="settings-host-work">Host working directory</Label><Input id="settings-host-work" v-model="model.host_runtime.work_dir" required />
      </template>
      <div class="flex items-center gap-2"><Checkbox id="settings-sandbox" :model-value="!!model.sandbox" @update:model-value="sandbox" /><Label for="settings-sandbox">Configure sandbox runtime</Label></div>
      <template v-if="model.sandbox">
        <div v-for="field in (['delegated_cgroup','runtime_dir','bwrap','ip','uv'] as const)" :key="field"><Label :for="`sandbox-${field}`">{{field.replaceAll('_',' ')}}</Label><Input :id="`sandbox-${field}`" v-model="model.sandbox[field]" required /></div>
      </template>
    </fieldset>
  </div>
</template>

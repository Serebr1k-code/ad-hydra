<script setup lang="ts">
import type { CheckerConfig } from '@/generated/contracts'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
const model = defineModel<CheckerConfig | null | undefined>({ required: true })
function select(value: unknown) {
  const kind = String(value)
  if (kind === 'none') { model.value = null; return }
  const common = { address: '127.0.0.1:31337', credential_configured: false }
  if (kind === 'http') model.value = { ...common, kind, host: 'localhost', path: '/', bearer: { mode: 'keep' } }
  if (kind === 'https') model.value = { ...common, kind, host: 'localhost', path: '/', bearer: { mode: 'keep' }, ca_certificate_der: [], ca_certificate_path: null }
  if (kind === 'tcp') model.value = { ...common, kind, secret: { mode: 'keep' } }
  if (kind === 'custom_tcp') model.value = { ...common, kind, secret: { mode: 'keep' }, request_template: '{items}\n', accepted_prefix: 'OK', batch_size: 32 }
}
function credential(event: Event) {
  if (!model.value) return
  const value = (event.target as HTMLInputElement).value
  const update = { mode: 'replace' as const, value: value || null }
  if ('bearer' in model.value) model.value.bearer = update
  else model.value.secret = update
}
</script>
<template>
  <fieldset class="space-y-3 rounded-md border p-4">
    <legend class="px-1 font-medium">Checker adapter</legend>
    <Label for="checker-kind">Protocol</Label>
    <Select :model-value="model?.kind || 'none'" @update:model-value="select"><SelectTrigger id="checker-kind"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">Not configured</SelectItem><SelectItem value="http">HTTP</SelectItem><SelectItem value="https">HTTPS</SelectItem><SelectItem value="tcp">TCP</SelectItem><SelectItem value="custom_tcp">Custom TCP</SelectItem></SelectContent></Select>
    <template v-if="model">
      <Label for="checker-address">Address (IP:port)</Label><Input id="checker-address" v-model="model.address" required />
      <template v-if="model.kind === 'http' || model.kind === 'https'">
        <Label for="checker-host">Host</Label><Input id="checker-host" v-model="model.host" required />
        <Label for="checker-path">Request path</Label><Input id="checker-path" v-model="model.path" required />
      </template>
      <template v-if="model.kind === 'https'">
        <Label for="checker-ca">CA certificate path (DER, optional)</Label><Input id="checker-ca" :model-value="model.ca_certificate_path || ''" @update:model-value="model.ca_certificate_path = String($event) || null; model.ca_certificate_der = []" />
        <p v-if="model.ca_certificate_der?.length" class="text-sm text-muted-foreground">An inline CA certificate is configured and will be preserved.</p>
      </template>
      <template v-if="model.kind === 'custom_tcp'">
        <Label for="checker-template">Request template (must include {items})</Label><Input id="checker-template" v-model="model.request_template" required />
        <Label for="checker-prefix">Accepted response prefix</Label><Input id="checker-prefix" v-model="model.accepted_prefix" required />
        <Label for="checker-batch">Batch size</Label><Input id="checker-batch" v-model.number="model.batch_size" type="number" min="1" max="32" required />
      </template>
      <Label for="checker-credential">Replace credential (optional)</Label><Input id="checker-credential" type="password" autocomplete="new-password" placeholder="Leave untouched to preserve saved credential" @input="credential" />
      <p class="text-sm text-muted-foreground">{{ model.credential_configured ? 'A credential is saved.' : 'No credential is saved.' }} An untouched field preserves it; entering and then clearing the field removes it.</p>
    </template>
  </fieldset>
</template>

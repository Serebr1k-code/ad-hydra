<script setup lang="ts">
import { onBeforeUnmount, onMounted, nextTick, reactive, ref } from 'vue'
import type { RetryAccepted, RetryInput, ServiceInput } from '@/generated/contracts'
import { api, ApiError, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import ForwardingForm from './ForwardingForm.vue'
import {forwardingDraft,validateForwarding} from './forwarding'
import ServiceProtectionForm from './ServiceProtectionForm.vue'
import { validateProtection, type ProtectionService, type ServiceProtection } from './protection'
import { tlsDraft, tlsPayload, validateTls } from './tls'
import { Textarea } from '@/components/ui/textarea'
import { validateEndpoints } from './endpoints'
import {decoderChoices,selectedDecoder,saveWithDecoder,type DecoderCatalog} from './decoders'

const props = defineProps<{ service?: Omit<ServiceInput, 'expected_revision'> & { id: string;name?:string } & ProtectionService; revision: number;initialSection?:'general'|'forwarding' }>()
const emit = defineEmits<{ saved: []; busy: [value: boolean] }>()
const initial = props.service
const form = reactive({
  name:initial?.name??initial?.id??'',
  transport: initial?.transport ?? 'tcp',
  listen_address: initial?.listen_address ?? '0.0.0.0:9000',
  upstream_address: initial?.upstream_address ?? '127.0.0.1:19000',
  protocol_id: initial?.protocol_id ?? 'raw',
  enabled: initial?.enabled ?? true,
  retain_history: initial?.retain_history ?? true,
})
const forwarding=ref(forwardingDraft(initial?.forwarding))
const protection = ref<ServiceProtection | null>(initial?.protection ? { ...initial.protection } : null)
const tls = ref(tlsDraft((initial as { tls?: Parameters<typeof tlsDraft>[0] } | undefined)?.tls))
function toggleTls(enabled: boolean) {
  if (enabled) {
    tls.value = tls.value ?? { terminate: true, upstream: false, insecure: false, certificate_pem: '', private_key_pem: '', server_name: '', private_key_configured: false }
    return
  }
  tls.value = null
}
const catalog=ref<DecoderCatalog>({items:[],activations:[]})
const loadingDecoders=ref(true), decodersReady=ref(false)
const savedId=ref(initial?.id), savedRevision=ref(props.revision)
const savedEnabled=ref(initial?.enabled ?? false)
onMounted(async()=>{try{catalog.value=await api<DecoderCatalog>('/protocols');decodersReady.value=true;if(initial)form.protocol_id=selectedDecoder(initial,catalog.value)}catch(cause){error.value=errorMessage(cause)}finally{loadingDecoders.value=false}})
onMounted(async()=>{if(props.initialSection==='forwarding'){await nextTick();requestAnimationFrame(()=>{const panel=document.getElementById('service-forwarding-settings');panel?.scrollIntoView({block:'start'});panel?.focus({preventScroll:true})})}})
const saving = ref(false)
const error = ref('')
const showRetry = ref(false)
const retrying = ref(false)
const retryGeneration = ref(0)
let retryTimer: ReturnType<typeof setTimeout> | undefined
onBeforeUnmount(() => { retryGeneration.value++; if (retryTimer) clearTimeout(retryTimer) })
async function save() {
  if (saving.value || !decodersReady.value) return
  error.value = !form.name.trim()||new TextEncoder().encode(form.name.trim()).length>256?'Enter a service name up to 256 UTF-8 bytes.':validateEndpoints(form.listen_address, form.upstream_address) || validateProtection(protection.value) || validateForwarding(forwarding.value) || validateTls(tls.value)
  if (error.value) return
  saving.value = true
  emit('busy', true)
  try {
    const input: ServiceInput & ProtectionService & {name?:string} = { ...form, name:form.name.trim(), forwarding:forwarding.value, protection: protection.value, tls: form.transport === 'tcp' ? tlsPayload(tls.value) : null, listen_address: form.listen_address.trim(), upstream_address: form.upstream_address.trim(), protocol_id: form.protocol_id.trim(), expected_revision: savedRevision.value }
    savedEnabled.value = input.enabled
    await saveWithDecoder(input,savedId.value,input.protocol_id,catalog.value,(id,revision)=>{savedId.value=id;savedRevision.value=revision})
    showRetry.value = false
    emit('saved')
  } catch (cause) {
    error.value = errorMessage(cause)
    showRetry.value = Boolean(savedId.value && savedEnabled.value && error.value.startsWith('Service saved;'))
  }
  finally { saving.value = false; emit('busy', false) }
}
function wait(ms: number) {
  return new Promise<void>(resolve => { retryTimer = setTimeout(() => { retryTimer = undefined; resolve() }, ms) })
}
async function retryListener() {
  if (!savedId.value || !savedEnabled.value || retrying.value || saving.value) return
  retrying.value = true
  const generation = ++retryGeneration.value
  error.value = ''
  try {
    const payload: RetryInput = { expected_revision: savedRevision.value }
    const result = await api<RetryAccepted>(`/services/${encodeURIComponent(savedId.value)}/retry`, 'POST', payload)
    if (generation !== retryGeneration.value) return
    error.value = `Retry requested for revision ${result.revision}. Pending listener and filter changes apply together.`
    for (let attempt = 0; attempt < 20 && generation === retryGeneration.value; attempt++) {
      await wait(750)
      if (generation !== retryGeneration.value) return
      const current = await api<{ revision: number; applied_revision: number; apply_status: string; apply_error?: string | null }>('/services')
      if (generation !== retryGeneration.value) return
      if (current.applied_revision >= result.revision && current.apply_status === 'applied') {
        error.value = 'Listener snapshot applied. If decoder activation is still pending, save the service again to retry that step.'
        break
      }
      if (current.apply_error) error.value = `${current.apply_error} Free the listener address, then retry.`
      if (attempt === 19) error.value = `${error.value} Retry remains pending; refresh Services to check again.`
    }
  } catch (cause) {
    if (generation !== retryGeneration.value) return
    if (cause instanceof ApiError && cause.status === 409) {
      try {
        const current = await api<{ revision: number }>('/services')
        if (generation !== retryGeneration.value) return
        savedRevision.value = current.revision
        error.value = `Service configuration changed. Refreshed to revision ${current.revision}; retry again. Your draft was not saved.`
      } catch { error.value = errorMessage(cause) }
    } else error.value = errorMessage(cause)
  } finally {
    if (generation === retryGeneration.value) retrying.value = false
  }
}
</script>

<template>
  <form id="service-form" class="space-y-4" @submit.prevent="save">
    <div class="space-y-2"><Label for="service-name">Name</Label><Input id="service-name" v-model="form.name" required maxlength="256" :disabled="saving" placeholder="e.g. Aquarius" /></div>
    <div class="space-y-2">
      <Label for="service-transport">Transport</Label>
      <Select v-model="form.transport" :disabled="saving">
        <SelectTrigger id="service-transport"><SelectValue /></SelectTrigger>
        <SelectContent><SelectItem value="tcp">TCP</SelectItem><SelectItem value="udp">UDP</SelectItem></SelectContent>
      </Select>
    </div>
    <div class="space-y-2">
      <Label for="service-listener">Public listener · original service port</Label>
      <Input id="service-listener" v-model="form.listen_address" required :disabled="saving" placeholder="0.0.0.0:8000" aria-describedby="service-listener-help" />
      <p id="service-listener-help" class="text-sm text-muted-foreground">Hydra listens here. Keep the port clients already use; select the public interface or 0.0.0.0 for all IPv4 interfaces.</p>
    </div>
    <div class="space-y-2">
      <Label for="service-backend">Application backend · relocated private port</Label>
      <Input id="service-backend" v-model="form.upstream_address" required :disabled="saving" placeholder="127.0.0.1:18000" aria-describedby="service-backend-help" />
      <p id="service-backend-help" class="text-sm text-muted-foreground">Reconfigure the local application to listen on loopback at a different port, then enter that endpoint here.</p>
    </div>
    <p class="rounded-md border p-3 text-sm">Before enabling this listener, move and restart the application on its backend port to free the original port. Hydra saves proxy configuration; you manage the application separately.</p>
    <div class="space-y-2">
      <Label for="service-profile">Capture decoder</Label>
      <Input id="service-profile" v-model="form.protocol_id" list="service-protocol-options" required :disabled="saving" placeholder="Search installed decoders" />
      <datalist id="service-protocol-options"><option v-for="choice in decoderChoices(catalog)" :key="choice.value" :value="choice.value">{{choice.label}}</option></datalist>
      <p class="text-sm text-muted-foreground">Upload a WASM module on Protocols, then select its version here. Saving applies the decoder to new connections; existing streams keep their decoder.</p>
    </div>
    <div class="flex items-center gap-2">
      <Checkbox id="service-enabled" v-model="form.enabled" :disabled="saving" />
      <Label for="service-enabled">Enable proxy listener</Label>
    </div>
    <div class="space-y-1">
      <div class="flex items-center gap-2">
        <Checkbox id="service-retain-history" v-model="form.retain_history" :disabled="saving" />
        <Label for="service-retain-history">Retain completed stream history</Label>
      </div>
      <p class="text-sm text-muted-foreground">Keeps a compressed tail (up to 64 KiB per direction) for 24 hours so finished TCP streams remain inspectable.</p>
    </div>
    <ForwardingForm v-model="forwarding" :disabled="saving" :transport="form.transport" :listen-address="form.listen_address" />
    <ServiceProtectionForm v-model="protection" :disabled="saving" :transport="form.transport" />
    <div v-if="form.transport === 'tcp'" class="space-y-2 rounded-md border p-3">
      <div class="flex items-center gap-2">
        <Checkbox id="service-tls-enabled" :model-value="tls !== null" :disabled="saving" @update:model-value="value => toggleTls(value === true)" />
        <Label for="service-tls-enabled">TLS</Label>
      </div>
      <template v-if="tls">
        <div class="flex items-center gap-2">
          <Checkbox id="service-tls-terminate" v-model="tls.terminate" :disabled="saving" />
          <Label for="service-tls-terminate">Terminate client TLS on this listener</Label>
        </div>
        <div class="flex items-center gap-2">
          <Checkbox id="service-tls-upstream" v-model="tls.upstream" :disabled="saving" />
          <Label for="service-tls-upstream">Connect to the backend over TLS</Label>
        </div>
        <div v-if="tls.upstream" class="flex items-center gap-2">
          <Checkbox id="service-tls-insecure" v-model="tls.insecure" :disabled="saving" />
          <Label for="service-tls-insecure">Skip backend certificate verification</Label>
        </div>
        <template v-if="tls.terminate">
          <div class="space-y-2">
            <Label for="service-tls-certificate">Certificate chain (PEM)</Label>
            <Textarea id="service-tls-certificate" v-model="tls.certificate_pem" class="min-h-[120px] font-mono text-xs" :disabled="saving" placeholder="-----BEGIN CERTIFICATE-----" />
          </div>
          <div class="space-y-2">
            <Label for="service-tls-key">Private key (PEM)</Label>
            <Textarea id="service-tls-key" v-model="tls.private_key_pem" class="min-h-[120px] font-mono text-xs" :disabled="saving" :placeholder="tls.private_key_configured ? 'Stored key kept; paste a new one to replace it' : '-----BEGIN PRIVATE KEY-----'" />
            <p class="text-sm text-muted-foreground">The key is stored with the service and never returned by the API. Leave it empty to keep the stored key.</p>
          </div>
        </template>
        <div v-if="tls.upstream" class="space-y-2">
          <Label for="service-tls-server-name">Backend certificate name (optional)</Label>
          <Input id="service-tls-server-name" v-model="tls.server_name" :disabled="saving" placeholder="Defaults to the backend IP address" />
        </div>
        <p class="text-sm text-muted-foreground">The Proxy offers HTTP/1.1 over TLS. HTTP/2-only backends stay unparsed. Terminating TLS exposes plaintext to this Proxy and to anyone who can read its memory.</p>
      </template>
    </div>
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    <div v-if="showRetry && savedId && savedEnabled" class="space-y-2 rounded-md border p-3">
      <p class="text-sm text-muted-foreground">If a listener bind failed, free the listener address, then retry. The retry leaves this form draft unsaved.</p>
      <Button id="service-retry-listener" type="button" variant="outline" :disabled="saving || retrying" @click="retryListener">{{ retrying ? 'Retrying…' : 'Retry listener' }}</Button>
    </div>
    <Button id="service-save" type="submit" :disabled="saving || retrying || !decodersReady">{{ saving ? 'Saving…' : 'Save service' }}</Button>
  </form>
</template>

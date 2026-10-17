<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import type { ServiceInput } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { validateEndpoints } from './endpoints'
import {decoderChoices,selectedDecoder,saveWithDecoder,type DecoderCatalog} from './decoders'

const props = defineProps<{ service?: Omit<ServiceInput, 'expected_revision'> & { id: string }; revision: number }>()
const emit = defineEmits<{ saved: []; busy: [value: boolean] }>()
const initial = props.service
const form = reactive({
  transport: initial?.transport ?? 'tcp',
  listen_address: initial?.listen_address ?? '0.0.0.0:9000',
  upstream_address: initial?.upstream_address ?? '127.0.0.1:19000',
  protocol_id: initial?.protocol_id ?? 'raw',
  enabled: initial?.enabled ?? true,
  retain_history: initial?.retain_history ?? true,
})
const catalog=ref<DecoderCatalog>({items:[],activations:[]})
const loadingDecoders=ref(true), decodersReady=ref(false)
const savedId=ref(initial?.id), savedRevision=ref(props.revision)
onMounted(async()=>{try{catalog.value=await api<DecoderCatalog>('/protocols');decodersReady.value=true;if(initial)form.protocol_id=selectedDecoder(initial,catalog.value)}catch(cause){error.value=errorMessage(cause)}finally{loadingDecoders.value=false}})
const saving = ref(false)
const error = ref('')
async function save() {
  if (saving.value || !decodersReady.value) return
  error.value = validateEndpoints(form.listen_address, form.upstream_address)
  if (error.value) return
  saving.value = true
  emit('busy', true)
  try {
    const input: ServiceInput = { ...form, listen_address: form.listen_address.trim(), upstream_address: form.upstream_address.trim(), protocol_id: form.protocol_id.trim(), expected_revision: savedRevision.value }
    await saveWithDecoder(input,savedId.value,input.protocol_id,catalog.value,(id,revision)=>{savedId.value=id;savedRevision.value=revision})
    emit('saved')
  } catch (cause) { error.value = errorMessage(cause) }
  finally { saving.value = false; emit('busy', false) }
}
</script>

<template>
  <form id="service-form" class="space-y-4" @submit.prevent="save">
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
      <p class="text-sm text-muted-foreground">Keeps a gzip-compressed tail (up to 64 KiB per direction) for 24 hours so finished TCP streams remain inspectable.</p>
    </div>
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    <Button id="service-save" type="submit" :disabled="saving || !decodersReady">{{ saving ? 'Saving…' : 'Save service' }}</Button>
  </form>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import type { Target } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
const props = defineProps<{ target: Target | null }>()
const emit = defineEmits<{ saved: []; cancel: [] }>()
const fresh = (): Target => ({ id: '', name: '', host: '', port: 9000, transport: 'tcp', enabled: true, revision: 0 })
const draft = ref<Target>(fresh())
const busy = ref(false), error = ref('')
watch(() => props.target, value => { draft.value = value ? { id: value.id, name: value.name, host: value.host, port: value.port, transport: value.transport, enabled: value.enabled, revision: value.revision } : fresh(); error.value = '' }, { immediate: true })
async function save() {
  busy.value = true; error.value = ''
  try {
    const value: Target = { ...draft.value, port: Number(draft.value.port) }
    await api('/farm/targets', 'PUT', value)
    emit('saved'); draft.value = fresh()
  } catch (e) { error.value = errorMessage(e) }
  finally { busy.value = false }
}
</script>
<template>
  <section class="space-y-3 rounded border p-4">
    <h2 class="text-lg font-semibold">{{ target ? 'Edit target' : 'Add target' }}</h2>
    <form class="grid gap-3 sm:grid-cols-2" @submit.prevent="save">
      <div><Label for="target-id">ID</Label><Input id="target-id" v-model="draft.id" required maxlength="128" :disabled="!!target || busy" /></div>
      <div><Label for="target-name">Name</Label><Input id="target-name" v-model="draft.name" required :disabled="busy" /></div>
      <div><Label for="target-host">Numeric IP address</Label><Input id="target-host" v-model="draft.host" required :disabled="busy" /></div>
      <div><Label for="target-port">Port</Label><Input id="target-port" :model-value="draft.port" @update:model-value="draft.port = Number($event)" type="number" min="1" max="65535" required :disabled="busy" /></div>
      <div><Label for="target-transport">Transport</Label><Select v-model="draft.transport" :disabled="busy"><SelectTrigger id="target-transport"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="tcp">TCP</SelectItem><SelectItem value="udp">UDP</SelectItem></SelectContent></Select></div>
      <div class="flex items-center gap-2"><Checkbox id="target-enabled" v-model="draft.enabled" :disabled="busy" /><Label for="target-enabled">Enabled</Label></div>
      <p v-if="error" role="alert" class="text-destructive sm:col-span-2">{{ error }}</p>
      <div class="flex gap-2"><Button :disabled="busy" type="submit">{{ busy ? 'Saving…' : 'Save target' }}</Button><Button v-if="target" type="button" variant="outline" :disabled="busy" @click="emit('cancel')">Cancel edit</Button></div>
    </form>
  </section>
</template>

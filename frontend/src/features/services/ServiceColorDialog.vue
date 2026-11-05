<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { ServiceColorInput } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { servicePalette, validServiceColor } from './colors'
const props = defineProps<{ service?: { id: string;name?:string }; color: string }>()
const emit = defineEmits<{ close: []; saved: [] }>()
const open = computed({ get: () => Boolean(props.service), set: value => { if (!value) emit('close') } })
const draft = ref(''), busy = ref(false), error = ref('')
watch(() => props.service, () => { draft.value = props.color; error.value = '' })
async function save(color: string | null) {
  if (!props.service || busy.value) return
  busy.value = true; error.value = ''
  try { const body:ServiceColorInput={color}; await api(`/services/${encodeURIComponent(props.service.id)}/color`, 'PUT', body); emit('saved'); emit('close') }
  catch (cause) { error.value = errorMessage(cause) } finally { busy.value = false }
}
</script>
<template>
  <Dialog v-model:open="open"><DialogContent><DialogHeader><DialogTitle>Service color</DialogTitle><DialogDescription>Choose a distinct color for {{ service?.name||service?.id }}. Everyone sees this color on Traffic.</DialogDescription></DialogHeader>
    <div class="grid grid-cols-4 gap-2"><Button v-for="preset in servicePalette" :key="preset.value" type="button" variant="outline" :aria-label="preset.name + ' service color'" :aria-pressed="draft.toLowerCase() === preset.value" class="h-auto flex-col gap-2 px-2 py-3" :class="draft.toLowerCase() === preset.value ? 'ring-2 ring-ring' : ''" @click="draft = preset.value"><span class="h-6 w-full rounded-sm" :style="{backgroundColor:preset.value}" aria-hidden="true"></span><span class="text-xs">{{ preset.name }}</span></Button></div>
    <div class="space-y-2"><Label for="service-custom-color">Custom color</Label><div class="flex gap-2"><Input aria-label="Pick custom service color" type="color" class="w-14 p-1" :model-value="validServiceColor(draft) ? draft : '#4dabf7'" @update:model-value="draft=String($event)"/><Input id="service-custom-color" v-model="draft" maxlength="7" placeholder="#RRGGBB" /></div></div>
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    <DialogFooter><Button variant="ghost" :disabled="busy" @click="save(null)">Reset to default</Button><Button variant="outline" :disabled="busy" @click="emit('close')">Cancel</Button><Button :disabled="busy || !validServiceColor(draft)" @click="save(draft)">Save color</Button></DialogFooter>
  </DialogContent></Dialog>
</template>

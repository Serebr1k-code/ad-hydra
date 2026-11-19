<script setup lang="ts">
import { ref, watch } from 'vue'
import DisclosureSection from '@/components/DisclosureSection.vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { api, errorMessage } from '@/shared/api/client'
import type { RequestCheckerPolicy, RequestCheckerSettings } from '@/generated/contracts'

const props = defineProps<{ services: Array<{ id: string; name: string }> }>()
const open = ref(false)
const settings = ref<RequestCheckerSettings>()
const busy = ref(false)
const notice = ref('')
const error = ref('')
const endpoint = '/filters/request-checkers'

async function load() {
  busy.value = true
  error.value = ''
  notice.value = ''
  try {
    settings.value = await api<RequestCheckerSettings>(endpoint)
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    busy.value = false
  }
}

async function save() {
  if (!settings.value || busy.value) return
  busy.value = true
  error.value = ''
  notice.value = ''
  try {
    settings.value = await api<RequestCheckerSettings>(endpoint, 'PUT', settings.value)
    notice.value = 'Request checker settings saved and applied.'
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    busy.value = false
  }
}

function options(policy: RequestCheckerPolicy) {
  const missing = policy.services.filter(id => !props.services.some(service => service.id === id))
  return [...props.services, ...missing.map(id => ({ id, name: `${id} (unavailable)` }))]
}

function usedByOther(policy: RequestCheckerPolicy, id: string) {
  return settings.value?.policies.some(other => other.id !== policy.id && other.services.includes(id))
}

function selectService(policy: RequestCheckerPolicy, id: string, selected: boolean) {
  policy.services = selected ? [...policy.services, id] : policy.services.filter(service => service !== id)
}

watch(open, expanded => {
  if (expanded && !settings.value && !busy.value) void load()
})
</script>

<template>
  <DisclosureSection v-model:open="open" title="Request checkers" description="Assign registered Python or WASM checkers to services and set their error handling.">
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    <p v-if="notice" role="status" class="text-sm">{{ notice }}</p>
    <p v-if="busy && !settings" role="status" class="text-sm text-muted-foreground">Loading checker settings…</p>
    <p v-if="settings && !settings.policies.length" class="text-sm text-muted-foreground">Register a Python script or WASM module in Proxy configuration to add a checker.</p>
    <form v-if="settings?.policies.length" class="space-y-4" @submit.prevent="save">
      <fieldset v-for="policy in settings.policies" :key="policy.id" :disabled="busy" class="space-y-3 rounded-md border p-3">
        <legend class="px-1 font-medium">{{ policy.id }}</legend>
        <div class="flex flex-wrap items-end gap-4">
          <label class="space-y-1 text-sm">
            <span>Timeout (ms)</span>
            <Input v-model.number="policy.timeout_ms" type="number" min="25" max="1000" required />
          </label>
          <div class="space-y-1 text-sm">
            <span>On timeout or error</span>
            <Select :model-value="policy.allow_on_error ? 'allow' : 'block'" :disabled="busy" @update:model-value="policy.allow_on_error = $event === 'allow'">
              <SelectTrigger :aria-label="`Error action for ${policy.id}`"><SelectValue /></SelectTrigger>
              <SelectContent><SelectItem value="allow">Allow traffic</SelectItem><SelectItem value="block">Block traffic</SelectItem></SelectContent>
            </Select>
          </div>
        </div>
        <div class="space-y-2">
          <p class="text-sm font-medium">Services</p>
          <div class="flex max-h-48 flex-wrap gap-3 overflow-y-auto">
            <label v-for="service in options(policy)" :key="service.id" class="flex items-center gap-2 text-sm">
              <Checkbox :model-value="policy.services.includes(service.id)" :disabled="busy || usedByOther(policy, service.id)" @update:model-value="selectService(policy, service.id, $event === true)" />
              {{ service.name }}
            </label>
          </div>
          <p class="text-xs text-muted-foreground">Clear the service selection to pause this checker and keep its files.</p>
        </div>
      </fieldset>
      <Button type="submit" :disabled="busy">Save checker settings</Button>
    </form>
    <Button type="button" variant="outline" :disabled="busy" @click="load">Reload checker settings</Button>
  </DisclosureSection>
</template>

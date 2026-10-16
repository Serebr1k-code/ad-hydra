<script setup lang="ts">
import { ref } from 'vue'
import { api, errorMessage } from '@/shared/api/client'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
const target = defineModel<string>('target', { required: true })
const emit = defineEmits<{ saved: [] }>()
const value = ref(''), error = ref(''), busy = ref(false)
async function save() {
  busy.value = true; error.value = ''
  try { await api('/flags', 'POST', { target_id: target.value, value: value.value }); value.value = ''; emit('saved') }
  catch (e) { error.value = errorMessage(e) }
  finally { busy.value = false }
}
</script>
<template>
  <section class="space-y-3 rounded border p-4">
    <h2 class="text-lg font-semibold">Record captured flag</h2>
    <form class="grid gap-3 sm:grid-cols-2" @submit.prevent="save">
      <div><Label for="flag-target">Enabled authorized target ID</Label><Input id="flag-target" v-model="target" required maxlength="128" :disabled="busy" /></div>
      <div><Label for="flag-value">Captured flag</Label><Input id="flag-value" v-model="value" type="password" autocomplete="off" required maxlength="256" :disabled="busy" /></div>
      <p class="text-sm text-muted-foreground sm:col-span-2">The target ID is also used for capture triage below. Flag values stay redacted in the vault.</p>
      <p v-if="error" role="alert" class="text-destructive sm:col-span-2">{{ error }}</p>
      <Button type="submit" :disabled="busy" class="w-fit">{{ busy ? 'Recording…' : 'Record captured flag' }}</Button>
    </form>
  </section>
</template>

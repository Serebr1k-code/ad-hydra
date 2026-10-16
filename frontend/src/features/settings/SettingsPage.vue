<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import type { SettingsView } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
const settings = ref<SettingsView>()
const loading = ref(false), exporting = ref(false), error = ref('')
const rows = computed(() => settings.value ? [
  { key: 'Flag format', value: settings.value.flag_pattern || 'Not configured' },
  { key: 'Automatic captured submission', value: String(settings.value.automatic_submission) },
  { key: 'Checker', value: settings.value.checker_adapter || 'Not configured' },
  { key: 'Checker batch size', value: String(settings.value.checker_batch_size) },
  { key: 'Sandbox configured', value: `${settings.value.sandbox_configured} (runs validate prerequisites)` },
  { key: 'Worker concurrency', value: String(settings.value.worker_concurrency) },
  { key: 'Worker networks', value: settings.value.supported_worker_networks.join(', ') },
  { key: 'Access token lifetime', value: `${settings.value.access_lifetime_seconds} seconds` },
  { key: 'Absolute refresh lifetime', value: `${settings.value.refresh_absolute_lifetime_seconds} seconds` },
] : [])
async function load() {
  loading.value = true; error.value = ''
  try { settings.value = await api<SettingsView>('/system/settings') }
  catch (e) { error.value = errorMessage(e) }
  finally { loading.value = false }
}
async function exportConfig() {
  exporting.value = true; error.value = ''
  try {
    const value = await api('/config/export')
    const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: 'application/json' }))
    const link = document.createElement('a'); link.href = url; link.download = 'hydra-config.json'; link.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  } catch (e) { error.value = errorMessage(e) }
  finally { exporting.value = false }
}
onMounted(load)
</script>
<template>
  <section class="space-y-4">
    <h1 class="text-2xl font-semibold">Settings</h1>
    <p class="text-muted-foreground">Current installation settings. Checker credentials and signing keys stay on the server. Operator configuration changes take effect after Core restart.</p>
    <Alert v-if="error" variant="destructive"><AlertDescription>{{ error }}</AlertDescription></Alert>
    <p v-if="loading" role="status">Loading settings…</p>
    <DataTable v-else-if="settings" :columns="[{key:'key',label:'Setting'},{key:'value',label:'Value'}]" :rows="rows" />
    <div class="flex flex-wrap gap-2"><Button variant="outline" :disabled="loading" @click="load">Refresh</Button><Button :disabled="exporting" @click="exportConfig">{{ exporting ? 'Exporting…' : 'Export service and filter configuration' }}</Button></div>
  </section>
</template>

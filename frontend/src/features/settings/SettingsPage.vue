<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, toRaw } from 'vue'
import type { SettingsView, EditableConfig } from '@/generated/contracts'
import { api, errorMessage } from '@/shared/api/client'
import ConfigurationForm from './ConfigurationForm.vue'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
const settings = ref<SettingsView>()
const draft = ref<EditableConfig>()
const saving = ref(false), notice = ref('')
const loading = ref(false), exporting = ref(false), error = ref('')
let timer:ReturnType<typeof setInterval>|undefined
let disposed=false,checking=false
async function checkHistory(){
 if(disposed||saving.value||loading.value||checking||!(settings.value?.history_apply_pending||settings.value?.compression_apply_pending))return
 checking=true
 try{const current=await api<SettingsView>('/system/settings');if(!disposed&&!saving.value)settings.value=current}catch(e){if(!disposed)error.value=errorMessage(e)}finally{checking=false}
}
const rows = computed(() => settings.value ? [
  { key:'Applied traffic history per service',value:settings.value.traffic_history_per_service_mib_applied==null?'Proxy unavailable / pending':`${settings.value.traffic_history_per_service_mib_applied} MiB` },
  { key:'Applied zstd level in Core',value:String(settings.value.core_compression_level_applied) },
  { key:'Applied zstd level in Proxy',value:settings.value.traffic_history_compression_level_applied==null?'Proxy unavailable / pending':String(settings.value.traffic_history_compression_level_applied) },
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
  try { settings.value = await api<SettingsView>('/system/settings'); draft.value = structuredClone(toRaw(settings.value.editable_config)) }
  catch (e) { error.value = errorMessage(e) }
  finally { loading.value = false }
}
async function save() {
  if (!settings.value?.saved_revision || !draft.value) return
  saving.value = true; error.value = ''; notice.value = ''
  try {
    settings.value = await api<SettingsView>('/system/settings', 'PUT', {saved_revision: settings.value.saved_revision, editable_config: draft.value})
    draft.value = structuredClone(toRaw(settings.value.editable_config))
    notice.value = 'Saved to the configuration file. Traffic retention and zstd level apply live.' + (settings.value.restart_required?' Restart Core to apply other changes.':'')
  } catch (cause) { error.value = errorMessage(cause) }
  finally { saving.value = false }
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
onMounted(()=>{void load();timer=setInterval(()=>{void checkHistory()},5000)})
onUnmounted(()=>{disposed=true;clearInterval(timer)})
</script>
<template>
  <section class="space-y-4">
    <h1 class="text-2xl font-semibold">Settings</h1>
    <p class="text-muted-foreground">Current installation settings. Checker credentials and signing keys stay on the server. Traffic retention and zstd compression apply live. Other operator configuration changes take effect after Core restart.</p>
    <Alert v-if="error" variant="destructive"><AlertDescription>{{ error }}</AlertDescription></Alert>
    <p v-if="loading" role="status">Loading settings…</p>
    <p v-if="notice" role="status">{{notice}}</p>
    <p v-if="settings?.history_apply_pending" role="status" class="text-sm text-muted-foreground">Saved history limit is pending Proxy connection or application.</p>
    <p v-if="settings?.compression_apply_pending" role="status" class="text-sm text-muted-foreground">Saved zstd compression level is pending Core and Proxy application.</p>
    <p v-if="settings?.restart_required" role="status" class="font-medium">Saved configuration is pending a Core restart.</p>
    <p v-if="settings?.readonly_reason" role="status">{{settings.readonly_reason}}</p>
    <form v-if="settings && draft" id="settings-form" class="space-y-4" @submit.prevent="save">
      <fieldset :disabled="saving || !settings.configuration_editable" class="space-y-4"><ConfigurationForm :key="settings.saved_revision || 'readonly'" v-model="draft" /><Button type="submit">{{saving ? 'Saving…' : 'Save settings'}}</Button></fieldset>
    </form>
    <h2 v-if="settings" class="text-lg font-semibold">Running configuration</h2>
    <DataTable v-if="settings" :columns="[{key:'key',label:'Setting'},{key:'value',label:'Value'}]" :rows="rows" />
    <div class="flex flex-wrap gap-2"><Button variant="outline" :disabled="loading" @click="load">Refresh</Button><Button :disabled="exporting" @click="exportConfig">{{ exporting ? 'Exporting…' : 'Export service and filter configuration' }}</Button></div>
  </section>
</template>

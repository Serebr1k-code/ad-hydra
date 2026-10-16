<script setup lang="ts">
import { onMounted, ref } from 'vue'
import type { Actor, AuditEvent, Session, User } from '@/generated/contracts'
import { api, clear, errorMessage } from '@/shared/api/client'
import { openEditor } from '@/shared/editor'
import { date } from '@/shared/format'
import DataTable from '@/components/DataTable.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Alert, AlertDescription } from '@/components/ui/alert'
const users = ref<User[]>([]), sessions = ref<Session[]>([]), audit = ref<AuditEvent[]>([])
const actor = ref<Actor>(), passwords = ref<Record<string,string>>({})
const loading = ref(false), busy = ref(''), auditLoading = ref(false), auditLoaded = ref(false), auditEnd = ref(false), error = ref('')
async function load() {
  loading.value = true; error.value = ''
  try { [users.value, actor.value, sessions.value] = await Promise.all([api<User[]>('/users'), api<Actor>('/auth/me'), api<Session[]>('/auth/sessions')]) }
  catch (e) { error.value = errorMessage(e) }
  finally { loading.value = false }
}
function signOutSelf(id: string) { if (id === actor.value?.id) { clear(); window.dispatchEvent(new Event('hydra-session-changed')); return true } return false }
function editUser(user: User) {
  openEditor('Account', { username: user.username, active: user.active }, async value => {
    await api(`/users/${encodeURIComponent(user.id)}`, 'PATCH', { ...value, expected_revision: user.revision })
    if (value.active === false && signOutSelf(user.id)) return
    await load()
  })
}
async function resetPassword(user: User) {
  busy.value = user.id; error.value = ''
  try {
    const password = passwords.value[user.id] || ''
    if (new TextEncoder().encode(password).length < 12) throw new Error('Password must contain at least 12 bytes')
    await api(`/users/${encodeURIComponent(user.id)}`, 'PATCH', { expected_revision: user.revision, password })
    passwords.value[user.id] = ''
    if (!signOutSelf(user.id)) await load()
  } catch (e) { error.value = errorMessage(e) }
  finally { busy.value = '' }
}
async function loadAudit(reset = false) {
  auditLoading.value = true; error.value = ''
  try {
    const cursor = reset ? 0 : (audit.value.at(-1)?.id || 0)
    const rows = await api<AuditEvent[]>(`/audit?after=${cursor}`)
    audit.value = rows; auditEnd.value = !rows.length; auditLoaded.value = true
  } catch (e) { error.value = errorMessage(e) }
  finally { auditLoading.value = false }
}
onMounted(load)
</script>
<template>
  <section class="space-y-4">
    <h1 class="text-2xl font-semibold">Users and audit</h1>
    <p class="text-muted-foreground">All active accounts have identical full privileges. Create or recover accounts through the local CLI. Password changes revoke every session of that account.</p>
    <Alert v-if="error" variant="destructive"><AlertDescription>{{ error }}</AlertDescription></Alert>
    <Button variant="outline" :disabled="loading" @click="load">{{ loading ? 'Loading…' : 'Refresh accounts' }}</Button>
    <DataTable :columns="[{key:'username',label:'Username'},{key:'active',label:'Active'},{key:'dates',label:'Created / updated'},{key:'actions',label:'Actions'}]" :rows="users">
      <template #cell-dates="{row}">{{ date(row.created_at) }} / {{ date(row.updated_at) }}</template>
      <template #cell-actions="{row}"><div class="flex min-w-64 flex-wrap gap-2"><Button size="sm" variant="outline" @click="editUser(row)">Edit account</Button><Input v-model="passwords[row.id]" type="password" autocomplete="new-password" :aria-label="'New password for '+row.username" placeholder="New password (12+ bytes)" /><Button size="sm" :disabled="!!busy" @click="resetPassword(row)">{{ busy === row.id ? 'Resetting…' : 'Reset password' }}</Button></div></template>
    </DataTable>
    <p v-if="!loading && !users.length">No accounts found.</p>
    <h2 class="text-lg font-semibold">Your sessions</h2>
    <p class="text-muted-foreground">Sign out to revoke the current session. Reset your password to revoke every session.</p>
    <DataTable :columns="[{key:'id',label:'Session'},{key:'expires_at',label:'Expires'},{key:'revoked',label:'Revoked'}]" :rows="sessions"><template #cell-expires_at="{row}">{{ date(row.expires_at) }}</template></DataTable>
    <p v-if="!loading && !sessions.length">No sessions found.</p>
    <h2 class="text-lg font-semibold">Audit history</h2>
    <div class="flex gap-2"><Button variant="outline" :disabled="auditLoading" @click="loadAudit(true)">Load audit</Button><Button variant="outline" :disabled="auditLoading || auditEnd" @click="loadAudit()">{{ auditLoading ? 'Loading…' : 'Next audit page' }}</Button></div>
    <DataTable v-if="auditLoaded" :columns="[{key:'id',label:'ID'},{key:'at',label:'Time'},{key:'actor_id',label:'Actor'},{key:'actor_kind',label:'Actor kind'},{key:'action',label:'Action'},{key:'target',label:'Target'},{key:'success',label:'Success'},{key:'request_id',label:'Request'},{key:'metadata',label:'Metadata'}]" :rows="audit"><template #cell-at="{row}">{{ date(row.at) }}</template></DataTable>
    <p v-if="auditEnd">No more audit events.</p>
  </section>
</template>

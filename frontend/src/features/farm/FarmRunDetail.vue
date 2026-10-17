<script setup lang="ts">
import type { FarmRunDetail } from './types'
import { Button } from '@/components/ui/button'
import { date } from '@/shared/format'
defineProps<{ run: FarmRunDetail; loading: boolean }>()
defineEmits<{ close: [] }>()
</script>
<template>
  <section class="space-y-3 rounded border p-4" aria-live="polite">
    <div class="flex flex-wrap items-center justify-between gap-2"><h2 class="text-lg font-semibold">Run {{ run.id }}</h2><Button variant="outline" @click="$emit('close')">Close logs</Button></div>
    <dl class="grid gap-2 text-sm sm:grid-cols-2"><div><dt class="text-muted-foreground">State</dt><dd>{{ run.status }}{{ loading ? ' · refreshing…' : '' }}</dd></div><div><dt class="text-muted-foreground">Exit code</dt><dd>{{ run.exit_code ?? 'Pending' }}</dd></div><div><dt class="text-muted-foreground">Created</dt><dd>{{ date(run.created_at) }}</dd></div><div><dt class="text-muted-foreground">Started / finished</dt><dd>{{ run.started_at ? date(run.started_at) : 'Not started' }} / {{ run.finished_at ? date(run.finished_at) : 'Not finished' }}</dd></div><div v-if="run.service_id"><dt class="text-muted-foreground">Service</dt><dd>{{ run.service_id }}</dd></div><div v-if="run.team_ids?.length"><dt class="text-muted-foreground">Teams</dt><dd>{{ run.team_ids.join(', ') }}</dd></div></dl>
    <p class="text-sm text-muted-foreground">Each output stream is limited to 1 MiB. Logs are compressed with gzip in storage and displayed as text here.</p>
    <div><h3 class="font-medium">Standard output <span v-if="run.stdout_truncated" class="text-destructive">(truncated at 1 MiB)</span></h3><pre class="max-h-96 overflow-auto whitespace-pre-wrap break-words rounded border p-3 text-sm">{{ run.stdout || '(No standard output)' }}</pre></div>
    <div><h3 class="font-medium">Standard error <span v-if="run.stderr_truncated" class="text-destructive">(truncated at 1 MiB)</span></h3><pre class="max-h-96 overflow-auto whitespace-pre-wrap break-words rounded border p-3 text-sm">{{ run.stderr || '(No standard error)' }}</pre></div>
  </section>
</template>

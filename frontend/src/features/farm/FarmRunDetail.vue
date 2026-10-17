<script setup lang="ts">
import type { FarmRunDetail } from './types'
import { Button } from '@/components/ui/button'
import { date } from '@/shared/format'
import FarmLogStream from './FarmLogStream.vue'
defineProps<{ run: FarmRunDetail; loading: boolean }>()
defineEmits<{ close: [] }>()
</script>
<template>
  <section class="space-y-3 rounded border p-4" aria-live="polite">
    <div class="flex flex-wrap items-center justify-between gap-2"><h2 class="text-lg font-semibold">Run {{ run.id }}</h2><Button variant="outline" @click="$emit('close')">Close logs</Button></div>
    <dl class="grid gap-2 text-sm sm:grid-cols-2"><div><dt class="text-muted-foreground">State</dt><dd>{{ run.status }}{{ loading ? ' · refreshing…' : '' }}</dd></div><div><dt class="text-muted-foreground">Exit code</dt><dd>{{ run.exit_code ?? 'Pending' }}</dd></div><div><dt class="text-muted-foreground">Created</dt><dd>{{ date(run.created_at) }}</dd></div><div><dt class="text-muted-foreground">Started / finished</dt><dd>{{ run.started_at ? date(run.started_at) : 'Not started' }} / {{ run.finished_at ? date(run.finished_at) : 'Not finished' }}</dd></div><div v-if="run.service_id"><dt class="text-muted-foreground">Service</dt><dd>{{ run.service_id }}</dd></div><div v-if="run.team_ids?.length"><dt class="text-muted-foreground">Teams</dt><dd>{{ run.team_ids.join(', ') }}</dd></div></dl>
    <p class="text-sm">New flags: <strong>{{run.new_flags??0}}</strong> · Existing flags: <strong>{{run.existing_flags??0}}</strong></p>
    <p v-if="run.flag_counts_complete===false" class="text-sm text-muted-foreground">Historical counts are partial: duplicate observations were not recorded before this update.</p>
    <ul v-if="run.flag_counts?.length" class="space-y-1 text-sm"><li v-for="item in run.flag_counts" :key="item.target_id">{{item.target_id}} · {{item.new_flags}} new · {{item.existing_flags}} existing</li></ul>
    <p class="text-sm text-muted-foreground">Gzip logs are paged in 64 KiB views. Output continues past 1 MiB; older retained output is available through the navigation buttons.</p>
    <FarmLogStream :run-id="run.id" stream="stdout" />
    <FarmLogStream :run-id="run.id" stream="stderr" />
  </section>
</template>

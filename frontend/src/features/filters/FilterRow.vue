<script setup lang="ts">
import { computed } from 'vue'
import { CheckCircle2, CircleAlert, Clock3, XCircle } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { username } from '@/shared/users/directory'
import { filterApplication, type FilterRule } from './filterState'
import { rowFromExpression } from './filterRow'
import { directionLabel } from './filterDirection'

const props = defineProps<{ filter: FilterRule; serviceName: string; serviceColor: string; selected: boolean; disabled: boolean; saving: boolean; applyStatus: string }>()
const emit = defineEmits<{ select: [checked: boolean]; state: [enabled: boolean]; edit: []; test: []; enforce: []; remove: [] }>()
const row = computed(() => rowFromExpression(props.filter.expression))
const pattern = computed(() => row.value?.pattern ?? '')
const direction = computed(() => directionLabel(row.value?.direction ?? 'both'))
const application = computed(() => filterApplication(props.filter, props.applyStatus))
const statusId = computed(() => `filter-application-${props.filter.id}`)
</script>

<template>
  <article data-filter-card :data-filter-id="filter.id" :data-service-id="filter.service_id" class="flex flex-wrap items-center gap-2 border-b border-l-4 px-2 py-0.5 text-[13px] last:border-b-0" :style="{ borderLeftColor: serviceColor }" :aria-label="filter.name">
    <Checkbox :model-value="selected" :disabled="disabled" :aria-label="'Select ' + filter.name" @update:model-value="emit('select', $event === true)" />
    <Switch :model-value="filter.desired_enabled" :disabled="disabled" :aria-label="'Enable ' + filter.name" :aria-describedby="statusId" :aria-busy="saving" @update:model-value="emit('state', $event)" />
    <span data-filter-desired :data-enabled="filter.desired_enabled" class="sr-only">{{ filter.desired_enabled ? 'Desired: enabled' : 'Desired: disabled' }}</span>
    <code v-if="pattern" class="max-w-80 truncate font-mono">{{ pattern }}</code>
    <span v-else class="text-muted-foreground">Advanced condition</span>
    <span class="max-w-60 truncate text-[11px] text-muted-foreground">{{ filter.name }}</span>
    <span class="rounded border px-1.5 text-[11px] leading-5 text-muted-foreground">{{ direction }}</span>
    <span v-if="filter.mode === 'shadow'" class="rounded border px-1.5 text-[11px] leading-5 text-muted-foreground">shadow</span>
    <dl class="ml-auto flex items-center gap-3 text-[11px] text-muted-foreground">
      <div><dt class="sr-only">Author</dt><dd>{{ username(filter.created_by) }}</dd></div>
      <div><dt class="sr-only">Last editor</dt><dd>{{ username(filter.updated_by) }}</dd></div>
      <div><dt class="sr-only">Hits</dt><dd class="tabular-nums">{{ filter.hit_count }}</dd></div>
    </dl>
    <span :id="statusId" data-filter-application :data-state="application.state" class="flex items-center gap-1.5 text-xs" :class="application.state === 'enabled' ? 'text-green-500' : application.state === 'disabled' || application.state === 'error' ? 'text-destructive' : 'text-muted-foreground'" aria-live="polite">
      <Clock3 v-if="saving || application.state === 'pending'" class="size-4" aria-hidden="true" /><CircleAlert v-else-if="application.state === 'error'" class="size-4" aria-hidden="true" /><CheckCircle2 v-else-if="application.state === 'enabled'" class="size-4" aria-hidden="true" /><XCircle v-else class="size-4" aria-hidden="true" />
      <span class="sr-only">{{ saving ? 'Saving desired state…' : application.label }}</span>
    </span>
    <div class="flex items-center gap-1">
      <Button variant="ghost" size="xs" :disabled="disabled" @click="emit('edit')">Edit</Button>
      <Button variant="ghost" size="xs" :disabled="disabled" @click="emit('test')">Test</Button>
      <Button v-if="filter.mode === 'shadow'" variant="ghost" size="xs" :disabled="disabled" @click="emit('enforce')">Enforce</Button>
      <Button variant="ghost" size="xs" class="text-destructive" :disabled="disabled" @click="emit('remove')">Delete</Button>
    </div>
  </article>
</template>

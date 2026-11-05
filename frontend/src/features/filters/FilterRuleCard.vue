<script setup lang="ts">
import { computed } from 'vue'
import {directionLabel} from './filterDirection'
import {fromExpression,conditionDirections} from './ruleBuilder'
import { CheckCircle2, CircleAlert, Clock3, XCircle } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { username } from '@/shared/users/directory'
import { date } from '@/shared/format'
import { filterApplication, type FilterRule } from './filterState'

const props = defineProps<{ filter: FilterRule; serviceName: string; serviceColor: string; selected: boolean; disabled: boolean; saving: boolean; applyStatus: string }>()
const emit = defineEmits<{ select: [checked: boolean]; state: [enabled: boolean]; edit: []; test: []; enforce: []; remove: [] }>()
const directions=computed(()=>{try{const sides=conditionDirections(fromExpression(props.filter.expression));return sides.length===1?directionLabel(sides[0]??'both'):'Direction varies by condition'}catch{return 'Advanced direction conditions'}})
const application = computed(() => filterApplication(props.filter, props.applyStatus))
const statusId = computed(() => `filter-application-${props.filter.id}`)
</script>

<template>
  <article data-filter-card :data-filter-id="filter.id" :data-service-id="filter.service_id" class="rounded-md border border-l-4 p-4" :style="{ borderLeftColor: serviceColor }" :aria-label="filter.name">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div class="flex min-w-0 items-start gap-3">
        <Checkbox class="mt-1" :model-value="selected" :disabled="disabled" :aria-label="'Select ' + filter.name" @update:model-value="emit('select', $event === true)" />
        <div class="min-w-0 space-y-1">
          <h2 class="break-words font-medium">{{ filter.name }}</h2>
          <p class="text-sm text-muted-foreground">{{ serviceName }} · {{ directions }}</p>
          <p class="text-sm">{{ filter.mode === 'shadow' ? 'Shadow — observes only; traffic passes' : 'Enforce — applies action to matches' }}</p>
        </div>
      </div>
      <div class="space-y-2">
        <div class="flex items-center gap-2">
          <Switch :model-value="filter.desired_enabled" :disabled="disabled" :aria-label="'Enable ' + filter.name" :aria-describedby="statusId" :aria-busy="saving" @update:model-value="emit('state', $event)" />
          <span data-filter-desired :data-enabled="filter.desired_enabled" class="flex items-center gap-1.5 text-sm font-medium" :class="filter.desired_enabled ? 'text-green-500' : 'text-destructive'">
            <CheckCircle2 v-if="filter.desired_enabled" class="size-4" aria-hidden="true" /><XCircle v-else class="size-4" aria-hidden="true" />
            Desired: {{ filter.desired_enabled ? 'enabled' : 'disabled' }}
          </span>
        </div>
        <p :id="statusId" data-filter-application :data-state="application.state" class="flex items-center gap-1.5 text-sm" :class="application.state === 'enabled' ? 'text-green-500' : application.state === 'disabled' || application.state === 'error' ? 'text-destructive' : 'text-muted-foreground'" aria-live="polite">
          <Clock3 v-if="saving || application.state === 'pending'" class="size-4" aria-hidden="true" /><CircleAlert v-else-if="application.state === 'error'" class="size-4" aria-hidden="true" /><CheckCircle2 v-else-if="application.state === 'enabled'" class="size-4" aria-hidden="true" /><XCircle v-else class="size-4" aria-hidden="true" />
          {{ saving ? 'Saving desired state…' : application.label }}
        </p>
        <p class="text-xs text-muted-foreground">Desired revision {{ filter.desired_revision }} / applied {{ filter.applied_revision }}</p>
      </div>
    </div>
    <dl class="mt-4 grid gap-x-6 gap-y-2 text-sm sm:grid-cols-2 lg:grid-cols-5">
      <div><dt class="text-xs text-muted-foreground">Priority</dt><dd>{{ filter.priority }}</dd></div>
      <div><dt class="text-xs text-muted-foreground">Created</dt><dd>{{ date(filter.created_at) }}</dd></div>
      <div><dt class="text-xs text-muted-foreground">Author</dt><dd>{{ username(filter.created_by) }}</dd></div>
      <div><dt class="text-xs text-muted-foreground">Last editor</dt><dd>{{ username(filter.updated_by) }}</dd></div>
      <div><dt class="text-xs text-muted-foreground">Hits</dt><dd>{{ filter.hit_count }}</dd></div>
    </dl>
    <div class="mt-4 flex flex-wrap gap-2">
      <Button variant="outline" size="sm" :disabled="disabled" @click="emit('edit')">Edit</Button>
      <Button variant="outline" size="sm" :disabled="disabled" @click="emit('test')">Test saved message</Button>
      <Button v-if="filter.mode === 'shadow'" variant="outline" size="sm" :disabled="disabled" @click="emit('enforce')">Enforce</Button>
      <Button variant="destructive" size="sm" :disabled="disabled" @click="emit('remove')">Delete</Button>
    </div>
  </article>
</template>

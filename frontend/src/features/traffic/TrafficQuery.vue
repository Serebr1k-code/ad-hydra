<script setup lang="ts">
import { reactive } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
const emit = defineEmits<{ search: [query: string] }>()
const fields = [
  ['source_ip', 'Source / client IP'], ['destination_ip', 'Destination / upstream IP'],
  ['since_ms', 'Since UTC'], ['until_ms', 'Until UTC'], ['field_path', 'Parser field (exact ID)'],
  ['field_value', 'Field value'], ['matched_rule', 'Matched rule ID'],
] as const
const choices = [
  { key: 'transport', label: 'Transport', options: [['1', 'TCP'], ['2', 'UDP']] },
  { key: 'direction', label: 'Direction', options: [['1', 'Client → server'], ['2', 'Server → client']] },
  { key: 'has_flag', label: 'Flag finding', options: [['true', 'Has flag finding'], ['false', 'No flag finding']] },
]
const values = reactive<Record<string, string>>({})
function search() {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(values)) {
    if (!value || value === 'all') continue
    if (key.endsWith('_ms')) query.set(key, String(Date.parse(value.length === 16 ? value + ':00Z' : value + 'Z')))
    else query.set(key, value)
  }
  emit('search', query.toString())
}
</script>
<template>
  <form class="space-y-3" @submit.prevent="search">
    <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
      <div v-for="[key,label] in fields" :key="key" class="space-y-1"><Label :for="'traffic-'+key">{{ label }}</Label><Input :id="'traffic-'+key" v-model="values[key]" :type="key.endsWith('_ms') ? 'datetime-local' : 'text'" /></div>
      <div v-for="choice in choices" :key="choice.key" class="space-y-1"><Label :for="'traffic-'+choice.key">{{ choice.label }}</Label><Select v-model="values[choice.key]"><SelectTrigger :id="'traffic-'+choice.key"><SelectValue :placeholder="choice.label" /></SelectTrigger><SelectContent><SelectItem value="all">All</SelectItem><SelectItem v-for="option in choice.options" :key="option[0]" :value="option[0] ?? ''">{{ option[1] }}</SelectItem></SelectContent></Select></div>
    </div>
    <Button type="submit">Search</Button>
  </form>
</template>

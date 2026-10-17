<script setup lang="ts">
import { fields, fieldKind, createCondition, createGroup, removeChild, type Condition, type Group, type RuleNode } from './ruleBuilder'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

defineProps<{ group: Group; depth?: number }>()
const emit = defineEmits<{ remove: [] }>()
const operators = {
  string: [
    { value: 'eq', label: 'equals' }, { value: 'ne', label: 'does not equal' },
    { value: 'contains', label: 'contains' }, { value: 'prefix', label: 'starts with' },
    { value: 'regex', label: 'matches regex' }, { value: 'not_regex', label: 'does not match regex' },
  ],
  number: [
    { value: 'eq', label: 'equals' }, { value: 'ne', label: 'does not equal' },
    { value: 'gt', label: 'is greater than' }, { value: 'lt', label: 'is less than' },
  ],
}
function setJoin(group: Group, value: unknown) { group.join = value === 'any' ? 'any' : 'all' }
function setField(condition: Condition, value: unknown) {
  condition.field = value === '__custom__' ? 'protocol.custom' : typeof value === 'string' ? value : condition.field
  condition.op = fieldKind(condition.field) === 'number' ? 'gt' : 'eq'
  condition.value = ''
}
function setOperator(condition: Condition, value: unknown) { if (typeof value === 'string') condition.op = value }
function add(group: Group, child: RuleNode) { group.children.push(child) }
function setCustomField(condition: Condition, value: string | number) { condition.field = String(value) }
function isKnownField(field: string) { return fields.some(item => item.value === field) }
function setNegate(node: Condition | Group, value: boolean | 'indeterminate') { node.negate = value === true }
</script>

<template>
  <div class="space-y-3 rounded-md border p-3" :class="depth ? 'ml-4 border-l-2' : ''">
    <div class="flex flex-wrap items-center gap-2">
      <span class="text-sm font-medium">Match</span>
      <Select :model-value="group.join" @update:model-value="value => setJoin(group, value)">
        <SelectTrigger class="w-32" aria-label="Group match mode"><SelectValue /></SelectTrigger>
        <SelectContent><SelectItem value="all">all (AND)</SelectItem><SelectItem value="any">any (OR)</SelectItem></SelectContent>
      </Select>
      <span class="text-sm text-muted-foreground">of these rules</span>
      <label class="ml-auto flex items-center gap-2 text-sm"><Checkbox :model-value="group.negate === true" aria-label="Invert group" @update:model-value="value => setNegate(group, value)" />Invert group</label>
      <Button v-if="depth" type="button" variant="ghost" size="sm" aria-label="Remove group" @click="emit('remove')">Remove group</Button>
    </div>
    <div v-for="(node, index) in group.children" :key="node.id" class="space-y-2">
      <template v-if="node.kind === 'condition'">
        <div class="grid gap-2 md:grid-cols-[1.1fr_1.1fr_1.2fr_auto]">
          <Select v-if="isKnownField(node.field)" :model-value="node.field" @update:model-value="value => setField(node, value)">
            <SelectTrigger aria-label="Rule field"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="field in fields" :key="field.value" :value="field.value">{{ field.label }}</SelectItem>
              <SelectItem value="__custom__">Custom protocol field…</SelectItem>
            </SelectContent>
          </Select>
          <Input v-else :model-value="node.field" aria-label="Custom protocol field path" placeholder="protocol.field" @update:model-value="value => setCustomField(node, value)" />
          <Select :model-value="node.op" @update:model-value="value => setOperator(node, value)">
            <SelectTrigger aria-label="Rule comparison"><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem v-for="op in operators[fieldKind(node.field)]" :key="op.value" :value="op.value">{{ op.label }}</SelectItem></SelectContent>
          </Select>
          <Input v-model="node.value" :aria-label="fieldKind(node.field) === 'number' ? 'Rule numeric value' : 'Rule text value'" :placeholder="fieldKind(node.field) === 'number' ? 'e.g. 512kb' : 'Value or regular expression'" />
          <Button type="button" variant="ghost" size="sm" aria-label="Remove rule" :disabled="group.children.length === 1" @click="removeChild(group, node.id)">Remove</Button>
        </div>
        <label v-if="node.op !== 'not_regex'" class="flex items-center gap-2 pl-1 text-xs text-muted-foreground"><Checkbox :model-value="node.negate === true" aria-label="Invert condition" @update:model-value="value => setNegate(node, value)" />Invert condition</label>
      </template>
      <RuleGroupEditor v-else :group="node" :depth="(depth ?? 0) + 1" @remove="removeChild(group, node.id)" />
      <p v-if="index < group.children.length - 1" class="pl-2 text-xs font-medium uppercase text-muted-foreground">{{ group.join === 'all' ? 'and' : 'or' }}</p>
    </div>
    <div class="flex flex-wrap gap-2">
      <Button type="button" variant="outline" size="sm" @click="add(group, createCondition())">Add condition</Button>
      <Button v-if="(depth ?? 0) < 5" type="button" variant="outline" size="sm" @click="add(group, createGroup())">Add group</Button>
    </div>
  </div>
</template>

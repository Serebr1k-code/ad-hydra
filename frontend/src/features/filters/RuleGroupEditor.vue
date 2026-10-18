<script setup lang="ts">
import { fields as defaultFields, fieldKind, createCondition, createGroup, removeChild, type Condition, type Group, type RuleField, type RuleNode } from './ruleBuilder'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

const props = withDefaults(defineProps<{ group: Group; fields?: RuleField[]; depth?: number }>(), { fields: () => defaultFields })
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
  boolean: [{ value: 'eq', label: 'equals' }, { value: 'ne', label: 'does not equal' }],
  bytes: [
    { value: 'eq', label: 'equals bytes' }, { value: 'ne', label: 'does not equal bytes' },
    { value: 'contains', label: 'contains bytes' }, { value: 'prefix', label: 'starts with bytes' },
  ],
}
function setJoin(group: Group, value: unknown) { group.join = value === 'any' ? 'any' : 'all' }
function setField(condition: Condition, value: unknown) {
  condition.field = value === '__custom__' ? 'protocol.custom' : typeof value === 'string' ? value : condition.field
  condition.op = fieldKind(condition.field, props.fields) === 'number' ? 'gt' : 'eq'
  condition.value = ''
}
function setOperator(condition: Condition, value: unknown) { if (typeof value === 'string') condition.op = value }
function add(group: Group, child: RuleNode) { group.children.push(child) }
function isKnownField(field: string) { return props.fields.some(item => item.value === field) }
function defaultField() { return props.fields.find(item => !defaultFields.some(base => base.value === item.value))?.value ?? props.fields[0]?.value ?? 'body.size' }
function conditionKind(field: string) { return fieldKind(field, props.fields) }
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
          <Select :model-value="node.field" @update:model-value="value => setField(node, value)">
            <SelectTrigger aria-label="Rule field"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-if="!isKnownField(node.field)" :value="node.field">Unavailable decoder field: {{ node.field }}</SelectItem>
              <SelectItem v-for="field in props.fields" :key="field.value" :value="field.value">{{ field.label }}</SelectItem>
            </SelectContent>
          </Select>
          <Select :model-value="node.op" @update:model-value="value => setOperator(node, value)">
            <SelectTrigger aria-label="Rule comparison"><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem v-for="op in operators[conditionKind(node.field)]" :key="op.value" :value="op.value">{{ op.label }}</SelectItem></SelectContent>
          </Select>
          <Select v-if="conditionKind(node.field) === 'boolean'" v-model="node.value"><SelectTrigger aria-label="Boolean rule value"><SelectValue placeholder="Choose true or false" /></SelectTrigger><SelectContent><SelectItem value="true">true</SelectItem><SelectItem value="false">false</SelectItem></SelectContent></Select>
          <Input v-else v-model="node.value" :aria-label="conditionKind(node.field) === 'number' ? 'Rule numeric value' : conditionKind(node.field) === 'bytes' ? 'Rule hex byte pattern' : 'Rule text value'" :placeholder="conditionKind(node.field) === 'number' ? 'e.g. 512kb' : conditionKind(node.field) === 'bytes' ? 'e.g. DE AD BE EF' : 'Value or regular expression'" />
          <Button type="button" variant="ghost" size="sm" aria-label="Remove rule" :disabled="group.children.length === 1" @click="removeChild(group, node.id)">Remove</Button>
        </div>
        <label v-if="node.op !== 'not_regex'" class="flex items-center gap-2 pl-1 text-xs text-muted-foreground"><Checkbox :model-value="node.negate === true" aria-label="Invert condition" @update:model-value="value => setNegate(node, value)" />Invert condition</label>
      </template>
      <RuleGroupEditor v-else :group="node" :fields="props.fields" :depth="(depth ?? 0) + 1" @remove="removeChild(group, node.id)" />
      <p v-if="index < group.children.length - 1" class="pl-2 text-xs font-medium uppercase text-muted-foreground">{{ group.join === 'all' ? 'and' : 'or' }}</p>
    </div>
    <div class="flex flex-wrap gap-2">
      <Button type="button" variant="outline" size="sm" @click="add(group, createCondition(defaultField(), props.fields))">Add condition</Button>
      <Button v-if="(depth ?? 0) < 5" type="button" variant="outline" size="sm" @click="add(group, createGroup('all', defaultField(), props.fields))">Add group</Button>
    </div>
  </div>
</template>

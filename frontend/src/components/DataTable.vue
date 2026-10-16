<script setup lang="ts" generic="T extends object">
import { Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '@/components/ui/table'
import { computed } from 'vue'
const props = defineProps<{ columns: {key:string;label:string}[]; rows: T[] }>()
const entries = computed(() => props.rows.map(row => ({ row, values: row as Record<string, unknown> })))
function text(value: unknown): string {
  if (value === null || value === undefined) return '—'
  return typeof value === 'object' ? JSON.stringify(value) : String(value)
}
</script>
<template>
 <div class="overflow-x-auto rounded-md border">
  <Table><TableHeader><TableRow><TableHead v-for="column in columns" :key="column.key">{{column.label}}</TableHead></TableRow></TableHeader>
   <TableBody><TableRow v-for="(entry,index) in entries" :key="`${entry.values.id ?? entry.values.flow_id ?? ''}:${index}`">
    <TableCell v-for="column in columns" :key="column.key" class="align-top whitespace-normal">
     <slot :name="`cell-${column.key}`" :row="entry.row" :column="column" :value="entry.values[column.key]">
      <slot name="cell" :row="entry.row" :column="column" :value="entry.values[column.key]">{{text(entry.values[column.key])}}</slot>
     </slot>
    </TableCell>
   </TableRow><TableRow v-if="!rows.length"><TableCell :colspan="columns.length" class="h-20 text-center text-muted-foreground">No records.</TableCell></TableRow></TableBody>
  </Table>
 </div>
</template>

<script setup lang="ts">
import { ChevronDown } from 'lucide-vue-next'
import { Collapsible,CollapsibleTrigger,CollapsibleContent } from '@/components/ui/collapsible'
import { Button } from '@/components/ui/button'
defineProps<{title:string;description?:string}>()
const open=defineModel<boolean>('open',{default:false})
</script>
<template><Collapsible v-model:open="open" class="rounded-md border"><CollapsibleTrigger as-child><Button variant="ghost" class="h-auto w-full justify-between gap-3 whitespace-normal p-4 text-left"><span><span class="block font-semibold">{{title}}</span><span v-if="description" class="block text-xs font-normal text-muted-foreground">{{description}}</span></span><ChevronDown class="size-4 shrink-0" :class="open?'rotate-180':''" /></Button></CollapsibleTrigger><CollapsibleContent class="space-y-3 border-t p-4"><slot /></CollapsibleContent></Collapsible></template>

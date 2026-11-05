<script setup lang="ts">
import { computed } from 'vue'
import { chartPaths, type MetricKey, type MetricPoint } from './metrics'
const props=defineProps<{points:MetricPoint[];color:string;title:string;series:{key:MetricKey;label:string;unit:string}[]}>()
const latest=computed(()=>props.points.at(-1))
function value(key:MetricKey) {const value=latest.value?.[key];return value == null?'Waiting for baseline':value.toLocaleString(undefined,{maximumFractionDigits:1})}
</script>
<template>
  <figure class="min-w-0 space-y-2">
    <figcaption class="text-sm font-medium">{{ title }}</figcaption>
    <svg viewBox="0 0 600 100" role="img" :aria-label="title + '. Latest values follow below.'" class="h-24 w-full overflow-visible">
      <path d="M0 90H600" class="stroke-border" fill="none" />
      <g v-for="(line,index) in series" :key="line.key"><path v-for="(path,i) in chartPaths(points,line.key)" :key="i" :d="path" fill="none" :stroke="color" :stroke-dasharray="index===0?undefined:index===1?'7 3':'2 3'" stroke-width="2" vector-effect="non-scaling-stroke" /></g>
    </svg>
    <dl class="space-y-1 text-sm"><div v-for="(line,index) in series" :key="line.key" class="flex flex-wrap justify-between gap-x-3"><dt>{{ line.label }} · {{ index===0?'solid':index===1?'dashed':'dotted' }}</dt><dd class="font-mono">{{ value(line.key) }} {{ latest?.[line.key] == null?'':line.unit }}</dd></div></dl>
    <p class="text-xs text-muted-foreground">Each line scaled independently; zero at the bottom. Breaks indicate missing samples or counter resets.</p>
  </figure>
</template>

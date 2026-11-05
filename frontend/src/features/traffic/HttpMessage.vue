<script setup lang="ts">
import {computed,ref} from 'vue'
import {Button} from '@/components/ui/button'
import {Table,TableBody,TableRow,TableCell} from '@/components/ui/table'
import {httpMessage} from './http'
import type {Message} from './traffic'
const props=defineProps<{message:Message}>()
const emit=defineEmits<{bytes:[]}>()
const view=computed(()=>httpMessage(props.message)),formatted=ref(true)
const sections=computed(()=>view.value?[{label:'Headers',...view.value.headers},{label:'Trailers',...view.value.trailers}]:[])
</script>
<template>
 <section v-if="view" data-http-message class="min-w-0 space-y-4" :aria-label="view.kind==='request'?'HTTP request':'HTTP response'">
  <div class="flex flex-wrap items-baseline gap-2"><h3 data-http-start-line class="min-w-0 break-all font-mono text-sm font-semibold" :class="view.kind==='request'?'text-red-300':'text-blue-300'">{{view.title}}</h3><span v-if="view.version" class="text-xs text-muted-foreground">{{view.version}}</span></div>
  <section v-if="view.query.items.length||view.query.truncated" class="space-y-1"><h4 class="text-xs font-medium">Query parameters <span class="text-muted-foreground">· decoded</span></h4><Table aria-label="HTTP query parameters"><TableBody><TableRow v-for="(item,index) in view.query.items" :key="index"><TableCell class="w-1/3 break-all font-mono text-xs text-muted-foreground">{{item.name}}</TableCell><TableCell class="break-all whitespace-pre-wrap font-mono text-xs">{{item.value}}</TableCell></TableRow></TableBody></Table><p v-if="view.query.truncated" class="text-xs text-muted-foreground">Query parameter preview limit reached.</p></section>
  <template v-for="section in sections" :key="section.label">
   <section v-if="section.label==='Headers'||section.items.length||section.truncated" class="space-y-1"><h4 class="text-xs font-medium">{{section.label}} <span class="text-muted-foreground">({{section.items.length}})</span></h4>
    <Table v-if="section.items.length" :aria-label="'HTTP '+section.label.toLowerCase()"><TableBody><TableRow v-for="(header,index) in section.items" :key="index"><TableCell class="w-1/3 break-all font-mono text-xs text-muted-foreground">{{header.name}}</TableCell><TableCell class="break-all whitespace-pre-wrap font-mono text-xs">{{header.value}}</TableCell></TableRow></TableBody></Table>
    <p v-else class="text-xs text-muted-foreground">{{section.available?'No headers.':'No header fields were retained.'}}</p><p v-if="section.truncated" class="text-xs text-muted-foreground">Header preview limit reached.</p>
   </section>
  </template>
  <section class="space-y-2"><div class="flex flex-wrap items-center justify-between gap-2"><h4 class="text-xs font-medium">Body <span class="text-muted-foreground">· {{view.bodyLength??'Unknown'}} bytes<span v-if="view.contentType"> · {{view.contentType}}</span></span></h4><div class="flex flex-wrap gap-1"><template v-if="view.prettyBody!==undefined"><Button size="sm" :variant="formatted?'secondary':'ghost'" :aria-pressed="formatted" @click="formatted=true">Formatted JSON</Button><Button size="sm" :variant="!formatted?'secondary':'ghost'" :aria-pressed="!formatted" @click="formatted=false">Text</Button></template><Button size="sm" variant="ghost" @click="emit('bytes')">Captured bytes</Button></div></div>
   <p v-if="view.truncated" role="status" class="text-xs text-amber-400">Body preview is truncated; captured bytes may contain more.</p>
   <pre data-http-body v-if="view.body!==undefined&&view.body!==''" class="!max-h-none break-words whitespace-pre-wrap font-mono !text-[13px] !leading-6">{{formatted&&view.prettyBody!==undefined?view.prettyBody:view.body}}</pre>
   <p v-else class="text-xs text-muted-foreground">{{view.bodyLength==='0'?'No body.':view.body===''?'Empty text preview.':'No text body preview was retained. Inspect Captured bytes for binary or encoded content.'}}</p>
  </section>
 </section>
</template>

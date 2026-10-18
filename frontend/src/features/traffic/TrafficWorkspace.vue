<script setup lang="ts">
import { useMediaQuery } from '@vueuse/core'
import { ResizablePanelGroup,ResizablePanel,ResizableHandle } from '@/components/ui/resizable'
const desktop=useMediaQuery('(min-width: 768px)')
</script>
<template>
 <ResizablePanelGroup v-if="desktop" direction="horizontal" class="h-full w-full">
  <ResizablePanel size-unit="px" :default-size="330" :min-size="240" :max-size="520" class="flex min-h-0"><slot name="sessions" /></ResizablePanel>
  <ResizableHandle />
  <ResizablePanel :min-size="25" class="flex min-h-0 min-w-0 flex-col"><slot /></ResizablePanel>
 </ResizablePanelGroup>
 <div v-else class="flex h-full min-h-0 flex-col"><slot name="sessions" /><div class="flex min-h-0 flex-1 flex-col"><slot /></div></div>
</template>

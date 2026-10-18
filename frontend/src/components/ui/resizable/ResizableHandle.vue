<script setup lang="ts">
import { SplitterResizeHandle } from 'reka-ui'
import { GripVertical } from 'lucide-vue-next'
</script>
<template><SplitterResizeHandle aria-label="Resize streams list" class="relative z-10 flex w-2 shrink-0 items-center justify-center bg-border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><GripVertical class="h-5 w-3 text-muted-foreground" /></SplitterResizeHandle></template>

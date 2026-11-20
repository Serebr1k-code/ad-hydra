<script setup lang="ts">
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import CodeEditor from '@/components/CodeEditor.vue'
import { editorOpen, editorTitle, editorText, editorBusy as busy, editorError as error, saveEditor, closeEditor } from '@/shared/editor'
import { errorMessage } from '@/shared/api/client'
const save = () => saveEditor(errorMessage)
</script>
<template>
 <Dialog :open="editorOpen" @update:open="!$event && closeEditor()"><DialogContent class="sm:max-w-3xl" @interact-outside="busy && $event.preventDefault()">
  <DialogHeader><DialogTitle id="editor-title">{{editorTitle}}</DialogTitle><DialogDescription>Edit the configuration, then save to apply the change.</DialogDescription></DialogHeader>
  <form @submit.prevent="save" class="space-y-4"><CodeEditor id="editor-value" v-model="editorText" language="json" aria-label="Configuration JSON" :disabled="busy" />
   <p id="editor-error" role="alert" class="text-destructive">{{error}}</p><DialogFooter><Button type="button" variant="outline" :disabled="busy" @click="closeEditor">Cancel</Button><Button id="editor-save" :disabled="busy">{{busy?'Saving…':'Save'}}</Button></DialogFooter>
  </form>
 </DialogContent></Dialog>
</template>

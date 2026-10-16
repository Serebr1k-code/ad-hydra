import { ref, shallowRef } from 'vue'
export const editorOpen = ref(false), editorTitle = ref(''), editorText = ref('')
export const editorBusy = ref(false), editorError = ref('')
export const editorSave = shallowRef<((value: Record<string,unknown>) => Promise<unknown>) | null>(null)
let generation = 0
export function closeEditor(): void {
 generation++; editorOpen.value = false; editorTitle.value = ''; editorText.value = ''; editorSave.value = null
 editorBusy.value = false; editorError.value = ''
}
export function openEditor(title: string, initial: object, save: (value: Record<string,unknown>) => Promise<unknown>): void {
 generation++; editorBusy.value = false; editorError.value = ''
 editorTitle.value = title; editorText.value = JSON.stringify(initial,null,2); editorSave.value = save; editorOpen.value = true
}
export async function saveEditor(formatError: (error: unknown) => string = error => error instanceof Error ? error.message : 'Request failed'): Promise<void> {
 if (!editorOpen.value || editorBusy.value || !editorSave.value) return
 const current = generation, save = editorSave.value
 editorBusy.value = true; editorError.value = ''
 try {
  const value: unknown = JSON.parse(editorText.value)
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Enter a JSON object')
  await save(value as Record<string,unknown>)
  if (generation === current) closeEditor()
 } catch (error) { if (generation === current) editorError.value = formatError(error) }
 finally { if (generation === current) editorBusy.value = false }
}

import { afterEach, expect, test } from 'bun:test'
import { closeEditor, editorBusy, editorError, editorOpen, editorText, editorTitle, openEditor, saveEditor } from '../src/shared/editor'
afterEach(closeEditor)
function deferred() {
 let resolve!: () => void, reject!: (error: Error) => void
 const promise = new Promise<void>((yes,no) => { resolve=yes; reject=no })
 return {promise,resolve,reject}
}
test('dismissed saves cannot close or change a newer editor', async () => {
 const old=deferred()
 openEditor('Old',{old:true},() => old.promise)
 const saving=saveEditor()
 closeEditor()
 openEditor('New',{new:true},async () => {})
 old.resolve(); await saving
 expect(editorOpen.value).toBe(true)
 expect(editorTitle.value).toBe('New')
 expect(editorText.value).toContain('new')
 expect(editorBusy.value).toBe(false)
 expect(editorError.value).toBe('')
})
test('old save failure cannot replace newer save state', async () => {
 const old=deferred(), current=deferred()
 openEditor('Old',{},() => old.promise); const savingOld=saveEditor()
 closeEditor(); openEditor('New',{},() => current.promise); const savingNew=saveEditor()
 old.reject(new Error('obsolete failure')); await savingOld
 expect(editorBusy.value).toBe(true); expect(editorError.value).toBe('')
 current.resolve(); await savingNew
 expect(editorOpen.value).toBe(false); expect(editorBusy.value).toBe(false)
})
test('closing clears editor content and pending callback', async () => {
 let saved=false
 openEditor('Sensitive',{secret:'example'},async () => { saved=true })
 closeEditor(); await saveEditor()
 expect(editorText.value).toBe(''); expect(editorTitle.value).toBe(''); expect(saved).toBe(false)
})

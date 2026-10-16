import { expect, test } from 'bun:test'
import { login, logout, clear } from '../src/shared/api/client'
import { closeEditor, editorOpen, editorSave, editorText, openEditor } from '../src/shared/editor'

test('successful local logout notifies the session cleanup listener', async () => {
 const originalFetch=globalThis.fetch
 const originalWindow=Object.getOwnPropertyDescriptor(globalThis,'window')
 const originalDocument=Object.getOwnPropertyDescriptor(globalThis,'document')
 const events=new EventTarget()
 let changes=0
 events.addEventListener('hydra-session-changed',() => { changes++; closeEditor() })
 Object.defineProperty(globalThis,'window',{value:events,configurable:true})
 Object.defineProperty(globalThis,'document',{value:{cookie:'hydra_csrf=test'},configurable:true})
 const requests:string[]=[]
 globalThis.fetch = (async (input: string | URL | Request) => {
  requests.push(String(input))
  return requests.length === 1 ? Response.json({access_token:'test-token',expires_in:60}) : new Response(null,{status:204})
 }) as typeof fetch
 try {
  await login('alice','test-password')
  openEditor('Sensitive',{value:'example'},async () => {})
  await logout()
  expect(requests).toEqual(['/api/v1/auth/login','/api/v1/auth/logout'])
  expect(changes).toBe(1)
  expect(editorOpen.value).toBe(false)
  expect(editorText.value).toBe('')
  expect(editorSave.value).toBe(null)
 } finally {
  globalThis.fetch=originalFetch; clear(); closeEditor()
  if (originalWindow) Object.defineProperty(globalThis,'window',originalWindow)
  else Reflect.deleteProperty(globalThis,'window')
  if (originalDocument) Object.defineProperty(globalThis,'document',originalDocument)
  else Reflect.deleteProperty(globalThis,'document')
 }
})

import { ref } from 'vue'
import { login, logout, refresh, errorMessage } from '@/shared/api/client'
export function useSession() {
 const authenticated = ref(false), busy = ref(false), error = ref('')
 async function initialize() {
  busy.value = true
  try { await refresh(); authenticated.value = true } catch { authenticated.value = false }
  finally { busy.value = false }
 }
 async function signIn(username: string, password: string) {
  busy.value = true; error.value = ''
  try { await login(username,password); authenticated.value = true }
  catch (e) { error.value = errorMessage(e) } finally { busy.value = false }
 }
 async function signOut() {
  busy.value = true; error.value = ''
  try { await logout(); authenticated.value = false }
  catch (e) { error.value = errorMessage(e) } finally { busy.value = false }
 }
 return { authenticated,busy,error,initialize,signIn,signOut }
}

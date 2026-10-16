import {path} from '/assets/operations.js';
let token = '', expires = 0, refreshing;
const sessions = new BroadcastChannel('hydra-session');
sessions.onmessage = () => { clear(); window.dispatchEvent(new Event('hydra-session-changed')); };
const serializeRefresh = operation => navigator.locks ? navigator.locks.request('hydra-refresh', operation) : operation();
const csrf = () => document.cookie.split(';').map(x => x.trim()).find(x => x.startsWith('hydra_csrf='))?.slice(11) || '';
export function clear() { token = ''; expires = 0; }
function remember(v) { token = v.access_token; expires = Date.now() + v.expires_in * 1000; }
export async function login(username, password) {
  const r = await fetch(path('login'), {method:'POST', credentials:'same-origin', headers:{'Content-Type':'application/json'}, body:JSON.stringify({username,password})});
  if (!r.ok) throw Error('Sign in failed'); remember(await r.json()); sessions.postMessage('login');
}
export async function refresh() {
  if (!refreshing) refreshing = serializeRefresh(async () => {
    const r = await fetch(path('refresh'), {method:'POST', credentials:'same-origin', headers:{'X-CSRF-Token':csrf()}});
    if (!r.ok) { clear(); throw Error('Please sign in'); } remember(await r.json());
  }).finally(() => {refreshing = undefined;});
  return refreshing;
}
export async function api(path, method='GET', value, retry=true, extraHeaders={}, raw=false) {
  if (!token || Date.now() > expires - 5000) await refresh();
  const headers = {...extraHeaders, 'Authorization':`Bearer ${token}`};
  if (value !== undefined) headers['Content-Type'] = raw ? 'application/octet-stream' : 'application/json';
  if (path === '/auth/logout') headers['X-CSRF-Token'] = csrf();
  const r = await fetch('/api/v1'+path, {method, credentials:'same-origin', headers, body:value === undefined ? undefined : (raw ? value : JSON.stringify(value))});
  if (r.status === 401 && retry) { await refresh(); return api(path,method,value,false,extraHeaders,raw); }
  if (!r.ok) { const e = await r.json().catch(() => ({})); const failure=Error(e.error?.message || `Request failed (${r.status})`);failure.status=r.status;failure.requestId=e.request_id;throw failure; }
  return r.status === 204 ? undefined : r.json();
}
export async function logout() { await api('/auth/logout','POST'); clear(); sessions.postMessage('logout'); }
export async function events(onEvent, signal) {
  while (!signal.aborted) {
    try {
      if (!token || Date.now() > expires - 5000) await refresh();
      const r = await fetch('/api/v1/traffic/events/stream',{headers:{Authorization:`Bearer ${token}`},signal});
      if (!r.ok) throw Error('Live events unavailable');
      const reader = r.body.getReader(), decoder = new TextDecoder(); let buffered = '';
      while (!signal.aborted) {
        const {done,value} = await reader.read(); if (done) break;
        buffered += decoder.decode(value,{stream:true});
        let boundary;
        while ((boundary = buffered.indexOf('\n\n')) >= 0) {
          const frame = buffered.slice(0,boundary); buffered = buffered.slice(boundary+2);
          if (frame.startsWith('event:')) onEvent(frame);
        }
        if (buffered.length > 131072) throw Error('Live event limit exceeded');
      }
    } catch(e) { if(signal.aborted) return; }
    if (!signal.aborted) await new Promise(resolve => setTimeout(resolve,1000));
  }
}
export async function uploadProtocol(id,version,file) {
 if(file.size>4*1024*1024)throw Error('WASM limit is 4 MiB');
 if(!token||Date.now()>expires-5000)await refresh();
 const body=await file.arrayBuffer(),digest=await crypto.subtle.digest('SHA-256',body),hash=[...new Uint8Array(digest)].map(x=>x.toString(16).padStart(2,'0')).join('');
 const r=await fetch(path('upload',{id,version}),{method:'PUT',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/wasm','X-SHA256':hash},body});
 const result=await r.json();if(!r.ok)throw Error(result.error?.message||'Upload failed');return result;
}
export async function activateProtocol(id,version,service_id) {
 if(!token||Date.now()>expires-5000)await refresh();
 const r=await fetch(path('activate',{id,version}),{method:'POST',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json','Idempotency-Key':crypto.randomUUID()},body:JSON.stringify({service_id})});
 const result=await r.json();if(!r.ok)throw Error(result.error?.message||'Activation failed');return result;
}

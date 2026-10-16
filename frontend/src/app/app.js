import {api,login,refresh,logout,events} from '/assets/api.js';
import {button,error} from '/assets/view.js';
import * as services from '/assets/services.js';
import * as filters from '/assets/filters.js';
import * as traffic from '/assets/traffic.js';
import * as users from '/assets/users.js';
import * as protocols from '/assets/protocols.js';
import * as settings from '/assets/settings.js';
import * as flags from '/assets/flags.js';
import * as farm from '/assets/farm.js';
const page=document.querySelector('#page'),status=document.querySelector('#status'),sections={Services:services,Filters:filters,Traffic:traffic,Protocols:protocols,Farm:farm,Flags:flags,'Users / audit':users,Settings:settings};
let active='Services',abort;
function signedOut(){abort?.abort();document.querySelector('#login').hidden=false;document.querySelector('#console').hidden=true;document.querySelector('#logout').hidden=true;status.textContent='Signed out';}
async function show(name){active=name;error();try{await sections[name].render(page);document.querySelectorAll('nav button').forEach(b=>b.setAttribute('aria-current',String(b.textContent===name)));}catch(e){error(e);}}
async function signedIn(){document.querySelector('#login').hidden=true;document.querySelector('#console').hidden=false;document.querySelector('#logout').hidden=false;await show(active);await updateStatus();abort?.abort();abort=new AbortController();events(frame=>{if(frame.startsWith('event: loss')){error(Error('Live events were dropped. Refresh traffic.'));return;}const line=frame.split('\n').find(x=>x.startsWith('data:'));if(line){try{const event=JSON.parse(line.slice(5));if(event.kind==='event_loss')error(Error('Inspection event: '+event.reason));}catch{}}if(active==='Traffic')status.textContent='Connected · new traffic available';},abort.signal);}
async function updateStatus(){try{const s=await api('/system/proxy');status.textContent=`Proxy ${s.connected?'connected':'disconnected'} · ${s.active_flows} flows · ${s.telemetry_dropped+s.indexing_dropped} telemetry lost · revision ${s.applied_services_revision}/${s.applied_rules_revision}`;}catch(e){error(e);}}
for(const name of Object.keys(sections))document.querySelector('#nav').append(button(name,()=>show(name)));
document.querySelector('#login-form').onsubmit=async event=>{event.preventDefault();const form=new FormData(event.target);try{await login(form.get('username'),form.get('password'));event.target.reset();await signedIn();}catch(e){error(e);}};
document.querySelector('#logout').onclick=async()=>{try{await logout();signedOut();}catch(e){error(e);}};
window.addEventListener('hydra-session-changed',signedOut);
setInterval(()=>{if(!document.querySelector('#console').hidden)updateStatus();},5000);
try{await refresh();await signedIn();}catch{signedOut();}

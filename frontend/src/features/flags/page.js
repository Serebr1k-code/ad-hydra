import {api} from '/assets/api.js';
import {el,button,table,date,error} from '/assets/view.js';
export async function render(page, offset=0, search="") {
 const [flags,submissions]=await Promise.all([api(`/flags?offset=${offset}&search=${encodeURIComponent(search)}`),api(`/submissions?offset=${offset}&search=${encodeURIComponent(search)}`)]);
 page.replaceChildren(el('h1','Flag Vault'));
 page.append(el('p','Values are redacted. Only captured flags from enabled authorized targets can be submitted. Proxy findings require triage and cannot be submitted.'));
 const query=el('input');query.placeholder='Search ID, digest, category or target';query.value=search;
 page.append(query,button('Search',()=>render(page,0,query.value)),button('Previous',()=>render(page,Math.max(0,offset-100),search)),button('Next',()=>render(page,offset+100,search)),el('p',`Entries ${offset+1}–${offset+100}`));
 const form=el('form'),target=el('input'),value=el('input'),status=el('p');
 target.required=true;target.maxLength=128;target.placeholder='Enabled target ID';target.setAttribute('aria-label','Enabled target ID');
 value.type='password';value.required=true;value.maxLength=256;value.autocomplete='off';value.placeholder='Captured flag';value.setAttribute('aria-label','Captured flag');
 const save=el('button','Record captured flag');save.type='submit';form.append(target,value,save,status);page.append(form);
 form.onsubmit=async event=>{event.preventDefault();save.disabled=true;try{await api('/flags','POST',{target_id:target.value,value:value.value});value.value='';await render(page);}catch(e){status.textContent=e.message;error(e);}finally{save.disabled=false;}};
 function triageActions(f) {
  const actions=el('span');
  for(const resolution of ['own_exposed','dismissed','captured'])actions.append(button(resolution,async()=>{
   try {const body={resolution};if(resolution==='captured'){body.target_id=target.value;if(!body.target_id){status.textContent='Enter an independently authorized enabled target ID before capture triage.';return;}}
   await api(`/flags/${encodeURIComponent(f.id)}/triage`,'POST',body);await render(page,offset,search);}catch(e){status.textContent=e.message;error(e);}
  }));return actions;
 }
 page.append(el('h2','Flags'),table(['Category','Digest','Target','Run','Initiated by','Created','Action'],flags.map(f=>[f.category,f.digest,f.target_id,f.run_id,f.initiated_by,date(f.created_at),f.category==='captured'?button('Submit',async()=>{await api(`/flags/${encodeURIComponent(f.id)}/submit`,'POST');await render(page);}):(f.category==='unknown'&&!f.triage?triageActions(f):f.triage||'Defensive finding')])));
 if(!flags.length)page.append(el('p','No flags recorded.','empty'));
 page.append(el('h2','Recent submissions'),table(['Flag ID','State','Attempts','Initiated by','Created','Checker outcome'],submissions.map(s=>[s.flag_id,s.state,s.attempts,s.initiated_by,date(s.created_at),s.response])));
 if(!submissions.length)page.append(el('p','No submissions queued.','empty'));
 page.append(el('p','Search and page through retained records. Retries preserve per-flag submission IDs; external exactly-once delivery requires checker idempotency support.'));
}

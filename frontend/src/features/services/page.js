import {api} from '/assets/api.js';
import {el,button,table,edit} from '/assets/view.js';
export async function render(page) {
 const data=await api('/services');page.replaceChildren(el('h1','Services'));
 const bar=el('div',undefined,'toolbar');bar.append(el('span',`Revision ${data.revision} · ${data.apply_status}${data.apply_error?' · '+data.apply_error:''}`));
 const save=async(id,value)=>{value.expected_revision=(await api('/services')).revision;await api('/services'+(id?'/'+id:''),id?'PATCH':'POST',value);await render(page);};
 bar.append(button('Add service',()=>edit('Create service',{transport:'tcp',listen_address:'127.0.0.1:9000',upstream_address:'127.0.0.1:9001',protocol_id:'raw',enabled:true},v=>save(null,v))),button('Refresh',()=>render(page)));page.append(bar);
 if(!data.items.length){page.append(el('p','No services configured. Add a listener and its defended upstream.','empty'));return;}
 page.append(table(['Transport','Listener','Upstream','Profile','Desired','Applied','Actions'],data.items.map(s=>{
  const actions=el('div');actions.append(button('Edit',()=>{const {transport,listen_address,upstream_address,protocol_id,enabled}=s;edit('Edit service',{transport,listen_address,upstream_address,protocol_id,enabled},v=>save(s.id,v));}),button('Delete',async()=>{if(confirm('Delete this service?')){await api(`/services/${s.id}?expected_revision=${(await api('/services')).revision}`,'DELETE');await render(page);}}));
  return[s.transport,s.listen_address,s.upstream_address,s.protocol_id,s.enabled?'On':'Off',s.applied_enabled===null?'Pending':s.applied_enabled?'On':'Off',actions];
 })));
}

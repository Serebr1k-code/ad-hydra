import {api} from '/assets/api.js';
import {el,button,table,edit,date} from '/assets/view.js';
export async function render(page,query='',after='') {
 const [data,services]=await Promise.all([api('/filters?q='+encodeURIComponent(query)+'&after='+encodeURIComponent(after)),api('/services')]);page.replaceChildren(el('h1','Filters'));
 const bar=el('div',undefined,'toolbar'),search=el('input');search.placeholder='Search filters';search.value=query;search.onchange=()=>render(page,search.value);
 const status=el('span',`${data.apply_status} · desired ${data.revision} / applied ${data.applied_revision}`);bar.append(search,status);
 const save=async(id,v)=>{v.expected_revision=(await api('/filters')).revision;await api('/filters'+(id?'/'+id:''),id?'PATCH':'POST',v);await render(page,query);};
 bar.append(button('Add filter',()=>edit('Create shadow filter',{service_id:services.items[0]?.id||'',name:'New filter',description:'',enabled:true,mode:'shadow',priority:10,expression:{field:'http.path',op:'prefix',value:{type:'string',value:'/blocked'}},action:{type:'block',http_status:403},backend:'userspace',expires_at_ms:null},v=>save(null,v))),button('Refresh',()=>render(page,query)));page.append(bar);
 if(data.apply_error)page.append(el('p',data.apply_error));
 if(!data.items.length){page.append(el('p','No filters. Start in shadow mode, inspect traffic, then test a saved message before enforcing.','empty'));return;}
 const sort=el('select');for(const [value,label] of [['created_at','Creation date'],['created_by','Author'],['hit_count','Hits'],['priority','Priority'],['name','Name']]){const o=el('option',label);o.value=value;sort.append(o);}bar.append(sort);
 const selected=new Set();
 const bulk=enabled=>async()=>{if(!selected.size)throw Error('Select filters first');await api('/filters/state','PUT',{ids:[...selected],enabled,expected_revision:data.revision});await render(page,query,after);};
 bar.append(button('Enable selected',bulk(true)),button('Disable selected',bulk(false)),button('Select page',()=>{for(const item of data.items)selected.add(item.id);draw();}));
 const target=el('div');page.append(target);
 if(after)page.append(button('First page',()=>render(page,query)));
 if(data.next_cursor)page.append(button('Next page',()=>render(page,query,data.next_cursor)));
 function draw(){const rows=[...data.items].sort((a,b)=>typeof a[sort.value]==='number'?b[sort.value]-a[sort.value]:String(a[sort.value]).localeCompare(String(b[sort.value])));target.replaceChildren(table(['Select','Name / mode','Priority','Created','Author / editor','Hits','Enabled','Actions'],rows.map(r=>{
  const actions=el('div');actions.append(button('Edit',()=>{const {id,created_at,updated_at,created_by,updated_by,hit_count,desired_revision,applied_revision,desired_enabled,applied_enabled,...value}=r;edit('Edit filter',value,v=>save(id,v));}),button('Test saved message',async()=>{const message_id=prompt('Saved parsed message ID (from Traffic)');if(message_id){const result=await api(`/filters/${r.id}/test`,'POST',{message_id});alert(result.matched?'Matched in shadow evaluation. Exact rule is now eligible for enforcement.':'No match on this saved message. Review the rule before enforcing.');}}),button('Delete',async()=>{if(confirm('Delete this filter?')){await api(`/filters/${r.id}?expected_revision=${(await api('/filters')).revision}`,'DELETE');await render(page,query);}}));
  const toggle=button(r.enabled?'Turn off':'Turn on',async()=>{await api(`/filters/${r.id}/state`,'PUT',{enabled:!r.enabled,expected_revision:(await api('/filters')).revision});await render(page,query);});
  const state=el('div');state.append(toggle,el('span',` Desired ${r.desired_enabled?'on':'off'} / applied ${r.applied_enabled===null?'pending':r.applied_enabled?'on':'off'}`));
  const checkbox=el('input');checkbox.type='checkbox';checkbox.checked=selected.has(r.id);checkbox.setAttribute('aria-label','Select '+r.name);checkbox.onchange=()=>checkbox.checked?selected.add(r.id):selected.delete(r.id);
  return[checkbox,`${r.name} · ${r.mode}`,r.priority,date(r.created_at),`${r.created_by}\n${r.updated_by}`,r.hit_count,state,actions];
 })));}sort.onchange=draw;draw();
}

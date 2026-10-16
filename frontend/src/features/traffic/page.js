import {api} from '/assets/api.js';
import {el,button,table,detail,edit} from '/assets/view.js';
export async function render(page,query='') {
 const data=await api('/traffic/flows'+query);page.replaceChildren(el('h1','Traffic'));
 const bar=el('div',undefined,'toolbar');
 const filters=new URLSearchParams(query),controls=[];
 for(const [key,label,type] of [['service_id','Service ID'],['source_ip','Source / client IP'],['destination_ip','Destination / upstream IP'],['since_ms','Since UTC','datetime-local'],['until_ms','Until UTC','datetime-local'],['field_path','Parser field (exact ID)'],['field_value','Field value'],['matched_rule','Matched rule ID']]){
   const input=el('input');input.type=type||'text';input.placeholder=label;input.setAttribute('aria-label',label);input.value=type?(filters.has(key)?new Date(Number(filters.get(key))).toISOString().slice(0,16):''):(filters.get(key)||'');controls.push([key,input,type]);bar.append(input);
 }
 for(const [key,label,options] of [['transport','Transport',[['1','TCP'],['2','UDP']]],['direction','Direction',[['1','Client → server'],['2','Server → client']]],['has_flag','Flag finding',[['true','Has flag finding'],['false','No flag finding']]]]){
   const select=el('select');select.setAttribute('aria-label',label);for(const [value,text] of [['',label],...options]){const o=el('option',text);o.value=value;select.append(o);}select.value=filters.get(key)||'';controls.push([key,select]);bar.append(select);
 }
 bar.append(button('Search',()=>{const q=new URLSearchParams();for(const [key,input,type] of controls)if(input.value)q.set(key,type?Date.parse(input.value+':00Z'):input.value);return render(page,'?'+q);}),button('Refresh',()=>render(page,query)));
 page.append(bar,el('p','Time filters use UTC. Source/destination refer to the opening client/upstream endpoints. Unknown protocols show flow metadata. Request a sample to capture the next chunk of a live flow; samples expire after 30 seconds.'));
 const selected=el('div');
 async function inspect(f){
   const flowPath=`/traffic/flows/${encodeURIComponent(f.flow_id)}`,[flow,m]=await Promise.all([api(flowPath),api(flowPath+'/messages')]);
   selected.replaceChildren(el('h2','Flow '+f.flow_id),detail(flow));
   const capture=el('div',undefined,'toolbar');
   capture.append(button('Capture next chunk',async()=>{
     const value=await api(flowPath+'/capture','POST');
     const result=el('div');
     result.append(el('p','Sample '+value.sample_id+' requested. Send traffic, then read the sample.'),button('Read sample',async()=>{
       try{const sample=await api(`/traffic/samples/${encodeURIComponent(value.sample_id)}`);const bytes=sample.data.slice(0,4096);result.replaceChildren(el('p',`Expires ${sample.expires_at}${sample.truncated?' · captured chunk truncated':''}`),el('pre',bytes.map(x=>x.toString(16).padStart(2,'0')).join(' ')),el('pre',bytes.map(x=>x>=32&&x<127?String.fromCharCode(x):'.').join('')),el('p',sample.data.length>4096?'Preview shows the first 4096 bytes.':''));}
       catch(e){if(e.status===410)result.replaceChildren(el('p','Sample is waiting for traffic, expired, or unavailable. Request another sample on a live flow.'));else throw e;}
     }));selected.append(result);
   }));
   selected.append(capture,el('h2','Bidirectional conversation'));
   let compare,shown=0;
   const comparison=el('div');
   function show(messages){
     for(const message of messages){
       if(shown++>=500)break;
       const card=el('article');card.append(el('h3',`${new Date(message.occurred_ms).toISOString()} · ${message.direction===1?'Client → server':message.direction===2?'Server → client':'Unknown direction'} · ${message.message_type||message.protocol_id}`),el('p','Message '+message.message_id));
       const fields=Object.entries(message.fields||{});
       card.append(table(['Field','Type','Value','Filter'],fields.map(([key,value])=>[key,value.type,JSON.stringify(value.value),button('Create shadow filter',()=>edit('Create filter from saved field',{message_id:message.message_id,field:key,name:'Saved '+key,action:{type:'alert'}},async input=>{input.expected_revision=(await api('/filters')).revision;await api('/filters/from-message','POST',input);alert('Shadow filter saved with the exact recorded value. Test it on the saved message in Filters before enforcement.');}))])));
       card.append(button('Select / compare message',()=>{
         if(!compare){compare=message;comparison.replaceChildren(el('p','Selected '+message.message_id+'. Choose another message to compare.'));return;}
         const keys=[...new Set([...Object.keys(compare.fields||{}),...Object.keys(message.fields||{})])].sort();
         comparison.replaceChildren(el('h3',compare.message_id+' ↔ '+message.message_id),table(['Field','Selected','Current','Difference'],keys.map(key=>{const a=JSON.stringify(compare.fields?.[key]),b=JSON.stringify(message.fields?.[key]);return[key,a??'Absent',b??'Absent',a===b?'Unchanged':a===undefined?'Added':b===undefined?'Removed':'Changed'];})));compare=undefined;
       }));selected.append(card);
     }
   }
   selected.append(comparison);show(m.items);
   if(!m.items.length)selected.append(el('p','No decoded messages indexed. Unknown or encrypted traffic has flow metadata only.'));
   if(m.next_cursor){let cursor=m.next_cursor;selected.append(button('More messages',async()=>{if(!cursor)return;if(shown>=500){alert('Conversation preview is limited to 500 messages. Narrow the flow query or use the paginated API.');return;}const next=await api(flowPath+'/messages?after='+cursor);show(next.items);cursor=next.next_cursor;}));}
 }
 page.append(table(['Service','Client','Started','Flow','Messages'],data.items.map(f=>[f.service_id,`${f.client?.ip||''}:${f.client?.port||''}`,new Date(f.occurred_ms).toLocaleString(),f.flow_id,button('Inspect',()=>inspect(f))])),selected);
 if(!data.items.length)page.append(el('p','No indexed flows yet. Send traffic through a configured listener.','empty'));
 if(data.next_cursor)page.append(button('Next page',()=>{const q=new URLSearchParams(query);q.set('after',data.next_cursor);return render(page,'?'+q);}));
}

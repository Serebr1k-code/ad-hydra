export function el(tag,text,cls) { const e=document.createElement(tag);if(text!==undefined)e.textContent=text;if(cls)e.className=cls;return e; }
export function button(label,action) { const b=el('button',label);b.onclick=async()=>{b.disabled=true;try{await action();}catch(e){error(e);}finally{b.disabled=false;}};return b; }
export function error(e) { document.querySelector('#error').textContent=e?.message || ''; }
export function date(value) { return value ? new Date(typeof value==='number'?value*1000:value).toLocaleString() : '—'; }
export function table(headers,rows) {const wrap=el('div',undefined,'table-wrap'),t=el('table'),head=el('tr'),body=el('tbody');headers.forEach(x=>head.append(el('th',x)));t.append(head,body);rows.forEach(values=>{const tr=el('tr');values.forEach(v=>{const td=el('td');if(v instanceof Node)td.append(v);else td.textContent=v??'—';tr.append(td);});body.append(tr);});wrap.append(t);return wrap;}
export function detail(value) {const pre=el('pre',JSON.stringify(value,null,2));return pre;}
export function edit(title,value,onSave) {
  const d=document.querySelector('#editor'),text=d.querySelector('textarea'),save=d.querySelector('#editor-save'),err=d.querySelector('#editor-error');
  document.querySelector('#editor-title').textContent=title;text.value=JSON.stringify(value,null,2);err.textContent='';
  save.onclick=async event=>{event.preventDefault();save.disabled=true;try{await onSave(JSON.parse(text.value));d.close();}catch(e){err.textContent=e.message;}finally{save.disabled=false;}};
  d.showModal();
}

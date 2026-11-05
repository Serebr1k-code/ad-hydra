import {record,type Message} from './traffic'
export interface HttpPair {name:string;value:string}
function stringField(message:Pick<Message,'fields'>,key:string):string|undefined{const f=record(message.fields[key]);return f.type==='string'&&typeof f.value==='string'?f.value:undefined}
function headerRows(field:unknown):{items:HttpPair[];truncated:boolean;available:boolean}{
 const source=record(field),items:HttpPair[]=[];let size=0,truncated=false
 if(source.type!=='list'||!Array.isArray(source.value))return {items,truncated,available:false}
 truncated=source.value.length>128
 for(const entry of source.value.slice(0,128)){const pair=record(record(entry).value),name=record(pair.name),value=record(pair.value);if(name.type!=='string'||value.type!=='string'||typeof name.value!=='string'||typeof value.value!=='string')continue
  size+=name.value.length+value.value.length;if(size>16384){truncated=true;break}items.push({name:name.value,value:value.value})
 }return {items,truncated,available:true}
}
/** Change whitespace only; numbers, duplicate keys and string escapes stay exact. */
function prettyJson(text:string|undefined,contentType:string):string|undefined{
 if(text===undefined||text.length>16384||(!contentType.toLowerCase().includes('json')&&!/^[\s]*[\[{]/.test(text)))return
 try{JSON.parse(text)}catch{return undefined}
 let output='',depth=0,quoted=false,escaped=false,previous=''
 const whitespace=(c:string|undefined)=>c===' '||c==='\n'||c==='\r'||c==='\t'
 const emit=(value:string)=>{if(output.length+value.length>65536)return false;output+=value;return true}
 for(let i=0;i<text.length;i++){
  const c=text[i]!
  if(quoted){if(!emit(c))return;if(escaped)escaped=false;else if(c==='\\')escaped=true;else if(c==='"'){quoted=false;previous='"'};continue}
  if(whitespace(c))continue
  if(c==='"'){quoted=true;if(!emit(c))return;previous=c;continue}
  if(c==='{'||c==='['){if(++depth>32)return;if(!emit(c))return;let next=i+1;while(whitespace(text[next]))next++;if(text[next]!=='}'&&text[next]!==']'&&!emit('\n'+'  '.repeat(depth)))return}
  else if(c==='}'||c===']'){depth--;if(previous!=='{'&&previous!=='['&&!emit('\n'+'  '.repeat(depth)))return;if(!emit(c))return}
  else if(c===','){if(!emit(',\n'+'  '.repeat(depth)))return}
  else if(c===':'){if(!emit(': '))return}
  else if(!emit(c))return
  previous=c
 }
 return output
}

export function isHttpMessage(message:Pick<Message,'fields'>):boolean{return Boolean(stringField(message,'http.method'))||/^[1-5][0-9]{2}$/.test(String(record(message.fields['http.status_code']).value??''))}
export function httpMessage(message:Pick<Message,'fields'|'truncated'>){
 if(!isHttpMessage(message))return undefined
 const method=stringField(message,'http.method'),target=stringField(message,'http.path')??'',status=String(record(message.fields['http.status_code']).value??''),reason=stringField(message,'http.reason')??'',version=stringField(message,'http.version')
 const headers=headerRows(message.fields['http.headers']),trailers=headerRows(message.fields['http.trailers']),query:{items:HttpPair[];truncated:boolean}={items:[],truncated:false}
 if(method&&target){try{const url=new URL(target,'http://hydra.invalid');for(const [name,value]of url.searchParams){if(query.items.length===128){query.truncated=true;break}query.items.push({name,value})}}catch{/* Keep the original target visible even if it is not a URL. */}}
 const body=stringField(message,'http.body_text'),contentType=stringField(message,'http.content_type')??headers.items.find(h=>h.name.toLowerCase()==='content-type')?.value.trim()??''
 const length=record(message.fields['http.body_length']).value
 return {kind:method?'request':'response',title:method?`${method} ${target}`.trim():`${status} ${reason}`.trim(),version,status,headers,trailers,query,body,prettyBody:prettyJson(body,contentType),contentType,bodyLength:typeof length==='string'||typeof length==='number'?String(length):undefined,truncated:message.truncated||record(message.fields['http.body_preview_truncated']).value===true}
}

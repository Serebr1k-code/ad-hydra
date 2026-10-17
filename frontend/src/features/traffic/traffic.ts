import type { FlowHistory, HistorySegment, Page } from '@/generated/contracts'
export function record(value: unknown): Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {} }
export function text(value: unknown) { return typeof value === 'string' || typeof value === 'number' ? String(value) : '' }
export function direction(value: unknown) { return value === 1 ? 'Client → server' : value === 2 ? 'Server → client' : 'Unknown direction' }
export function timestamp(value: unknown) { return typeof value === 'number' && Number.isFinite(value) ? new Date(value).toISOString() : 'Unknown time' }
export function endpoint(value: unknown) { const v=record(value), ip=text(v.ip); return ip ? `${ip.includes(':')?'['+ip+']':ip}:${text(v.port)}` : 'Unknown endpoint' }
export function flowRows(page: Page) { return page.items.map(item => ({ ...item, closed:item.closed, flow_id:text(item.flow_id), service_id:text(item.service_id), client:endpoint(item.client), destination:endpoint(item.upstream), started:timestamp(item.occurred_ms), state:item.closed?'Closed':'Open', bytesUp:text(record(item.closed).bytes_client_to_server), bytesDown:text(record(item.closed).bytes_server_to_client) })) }
export type Flow = ReturnType<typeof flowRows>[number]
export function mergeFlows(current: Flow[], incoming: Flow[], older = false): Flow[] {
 const unique = new Map<string, Flow>()
 for (const flow of older ? [...current,...incoming] : [...incoming,...current]) {
  if (!unique.has(flow.flow_id)) unique.set(flow.flow_id,flow)
 }
 return [...unique.values()].slice(0,500)
}
export function messageRows(page: Page) { return page.items.map(item => ({ ...item, message_id:text(item.message_id), occurred_ms:item.occurred_ms, direction:item.direction, message_type:text(item.message_type), protocol_id:text(item.protocol_id), fields:record(item.fields), truncated:item.truncated===true })) }
export type Message = ReturnType<typeof messageRows>[number]
export function initialStreamMode(protocolId: string): 'bytes'|'decoded' { return protocolId==='raw'?'bytes':'decoded' }
export function fields(message: Pick<Message,'fields'>) { return Object.entries(message.fields).map(([key,value])=>({key,type:text(record(value).type),value:JSON.stringify(record(value).value)??''})) }
/** Retained HTTP headers, preserving observed spelling, duplicates and whitespace. */
export function httpHeaderLines(field: unknown): string[] | undefined {
 const list=record(field)
 if(list.type!=='list'||!Array.isArray(list.value))return undefined
 const lines:string[]=[];let remaining=16384
 for(const item of list.value.slice(0,128)) {
  const object=record(item),header=record(object.value),name=record(header.name),value=record(header.value)
  if(object.type!=='object'||name.type!=='string'||typeof name.value!=='string'||value.type!=='string'||typeof value.value!=='string')continue
  const line=`${name.value}:${value.value}`
  if(line.length+2>remaining){lines.push('[headers truncated]');return lines}
  lines.push(line);remaining-=line.length+2
 }
 if(list.value.length>128)lines.push('[headers truncated]')
 return lines
}
/** Decoded fields only. This is not a reconstructed wire payload. */
export function streamText(message: Pick<Message,'fields'>): string {
 const lines:string[]=[];let length=0,truncated=false
 for(const [key,field] of Object.entries(message.fields)) {
  const headers=key==='http.headers'||key==='http.trailers'?httpHeaderLines(field):undefined
  if(headers){lines.push(`${key}:`,...headers);continue}
  if(truncated)continue
  const value=record(field).value
  const line=`${key}: ${typeof value==='string'?value:JSON.stringify(value)??''}`
  lines.push(line.slice(0,Math.max(0,8192-length)));length+=line.length+1
  if(length>8192){lines.push('[preview truncated]');truncated=true}
 }
 return lines.join('\n')||'(no decoded fields)'
}
export function bytePreview(data:number[]) {
 const bytes=data,lines:string[]=[]
 const ascii=bytes.map(b=>b===9||b===10||b===13||b>=32&&b<127?String.fromCharCode(b):'·').join('')
 const utf8=new TextDecoder('utf-8').decode(new Uint8Array(bytes))
 for(let offset=0;offset<bytes.length;offset+=16){const row=bytes.slice(offset,offset+16);lines.push(`${offset.toString(16).padStart(8,'0')}  ${row.map(b=>b.toString(16).padStart(2,'0')).join(' ').padEnd(47,' ')}  ${row.map(b=>b>=32&&b<127?String.fromCharCode(b):'.').join('')}`)}
 return {ascii,hex:lines.join('\n'),utf8}
}
export type HistoryDisplaySegment = HistorySegment & { truncated: boolean }
/** Preserves arrival order while optionally limiting a saved stream to one direction. */
export function historySegments(history: Pick<FlowHistory,'segments'|'truncated_client_to_server'|'truncated_server_to_client'>, side: 'both'|'1'|'2'): HistoryDisplaySegment[] {
 return history.segments.filter(segment=>side==='both'||String(segment.direction)===side).map(segment=>({...segment,truncated:segment.direction===1?history.truncated_client_to_server:history.truncated_server_to_client}))
}
/** Combine saved chunks per direction so split UTF-8 code points decode correctly. */
export function historyTextSegments(history: Pick<FlowHistory,'segments'|'truncated_client_to_server'|'truncated_server_to_client'>, side: 'both'|'1'|'2'): HistoryDisplaySegment[] {
 const grouped=new Map<number,HistoryDisplaySegment>()
 for(const segment of historySegments(history,side)) {
  const existing=grouped.get(segment.direction)
  if(existing){existing.data.push(...segment.data);existing.truncated ||= segment.truncated}
  else grouped.set(segment.direction,{...segment,data:[...segment.data]})
 }
 return [...grouped.values()]
}
/** Raw defaults to the hex dump; text decoding is an explicit operator choice. */
export function historyPreview(segment: Pick<HistorySegment,'data'>, mode: 'hex'|'utf8'|'ascii'): string {
 const preview=bytePreview(segment.data)
 return mode==='utf8'?preview.utf8:mode==='ascii'?preview.ascii:preview.hex
}
export function differences(a:Pick<Message,'fields'>,b:Pick<Message,'fields'>) {
 return [...new Set([...Object.keys(a.fields),...Object.keys(b.fields)])].sort().map(key=>{
  const left=JSON.stringify(a.fields[key]),right=JSON.stringify(b.fields[key])
  return {key,left:left??'Absent',right:right??'Absent',change:left===right?'Unchanged':left===undefined?'Added':right===undefined?'Removed':'Changed'}
 })
}

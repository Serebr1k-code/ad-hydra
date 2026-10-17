import { flowRows, messageRows, record, type Flow, type Message } from './traffic'
import { parseEventFrame } from '@/shared/api/sse'
export type TrafficEvent = Record<string, unknown> & { kind: string; flow_id: string; service_id: string }
export function trafficEvent(frame: string): TrafficEvent | undefined {
 const parsed=parseEventFrame(frame)
 if(parsed?.event!=='traffic')return
 const event=record(JSON.parse(parsed.data))
 if(typeof event.kind!=='string'||typeof event.flow_id!=='string'||typeof event.service_id!=='string')throw new Error('Invalid live traffic event')
 return event as TrafficEvent
}
/** Apply committed parsed events directly; no timer or REST round trip per message. */
export function updateFlows(rows: Flow[],event:TrafficEvent):Flow[] {
 if(event.kind==='flow_opened') {
  const flow=flowRows({items:[event],next_cursor:null})[0]
  if(!flow)return rows
  const old=rows.find(r=>r.flow_id===flow.flow_id)
  return [old?.closed?{...flow,closed:old.closed,state:old.state,bytesUp:old.bytesUp,bytesDown:old.bytesDown}:flow,...rows.filter(r=>r.flow_id!==flow.flow_id)].slice(0,500)
 }
 if(event.kind==='flow_closed')return rows.map(row=>row.flow_id===event.flow_id?{...row,closed:event,state:'Closed',bytesUp:String(event.bytes_client_to_server??''),bytesDown:String(event.bytes_server_to_client??'')}:row)
 return rows
}
export function mergeMessages(current:Message[],incoming:Message[]):Message[] {
 const byId=new Map(current.map(m=>[m.message_id,m]))
 for(const message of incoming)byId.set(message.message_id,message)
 return [...byId.values()].sort((a,b)=>Number(a.occurred_ms??0)-Number(b.occurred_ms??0)).slice(-500)
}
export function eventMessage(event:TrafficEvent):Message|undefined {
 return event.kind==='message'?messageRows({items:[event],next_cursor:null})[0]:undefined
}

import { expect,test } from 'bun:test'
import { trafficEvent,updateFlows,eventMessage,mergeMessages } from '../src/features/traffic/live'
import { parseEventFrame } from '../src/shared/api/sse'
const opened={kind:'flow_opened',flow_id:'f',service_id:'s',transport:1,occurred_ms:1,client:{ip:'127.0.0.1',port:1},upstream:{ip:'127.0.0.1',port:2}}
test('live flow opens and closes with exact uint byte counters',()=>{
 const event=trafficEvent('event: traffic\ndata: '+JSON.stringify(opened))!
 const rows=updateFlows([],event)
 expect(rows[0]?.flow_id).toBe('f')
 const closed=updateFlows(rows,{...opened,kind:'flow_closed',bytes_client_to_server:'18446744073709551615',bytes_server_to_client:'5'})
 expect(closed[0]?.state).toBe('Closed')
 expect(closed[0]?.bytesUp).toBe('18446744073709551615')
 expect(updateFlows(closed,event)[0]?.state).toBe('Closed')
})
test('live messages merge with initial query without duplicates, in order and bounded',()=>{
 const event={...opened,kind:'message',message_id:'m',protocol_id:'http1',direction:2,message_type:'response',fields:{'http.status_code':{type:'uint',value:'200'}}}
 const message=eventMessage(event)!
 expect(mergeMessages([message],[message])).toHaveLength(1)
 expect(message.direction).toBe(2)
 const rows=Array.from({length:501},(_,i)=>({...message,message_id:String(i),occurred_ms:i}))
 const merged=mergeMessages([],rows)
 expect(merged).toHaveLength(500)
 expect(merged[0]?.message_id).toBe('1')
 expect(eventMessage(opened)).toBeUndefined()
})
test('SSE parses loss, multiline data, ignores keepalive and rejects malformed traffic',()=>{
 expect(parseEventFrame(': keepalive')).toBeUndefined()
 expect(parseEventFrame('event: loss\ndata: 7')).toEqual({event:'loss',data:'7'})
 expect(parseEventFrame('event: traffic\ndata: {\ndata: }')).toEqual({event:'traffic',data:'{\n}'})
 expect(()=>trafficEvent('event: traffic\ndata: {}')).toThrow('Invalid')
 expect(trafficEvent('event: loss\ndata: 7')).toBeUndefined()
})

test('blocking and flag events update cards without confusing shadow hits with blocking',()=>{
 const rows=updateFlows([],{...opened})
 const shadow=updateFlows(rows,{...opened,kind:'rule_hit',mode:1,action:3})
 expect(shadow[0]?.blocked).toBe(false)
 const flagged=updateFlows(shadow,{...opened,kind:'flag_detected',has_flag:true})
 expect(flagged[0]?.has_flag).toBe(true)
 const blocked=updateFlows(flagged,{...opened,kind:'rule_hit',mode:2,action:6,blocked:true})
 expect(blocked[0]?.blocked).toBe(true)
 expect(updateFlows(blocked,{...opened})[0]).toMatchObject({blocked:true,has_flag:true})
})

test('UDP conversations participate in the same live list and preserve transport identity',()=>{
 const udp={...opened,flow_id:'udp',transport:2}
 const mixed=updateFlows(updateFlows([],opened),udp)
 expect(mixed.map(row=>row.transport)).toEqual([2,1])
 const closed=updateFlows(mixed,{...udp,kind:'flow_closed',bytes_client_to_server:'5',bytes_server_to_client:'7'})
 expect(closed[0]?.transport).toBe(2)
 expect(closed[0]?.bytesDown).toBe('7')
})

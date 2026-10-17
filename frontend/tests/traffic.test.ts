import { expect, test } from 'bun:test'
import * as traffic from '../src/features/traffic/traffic'

test('live session refresh keeps newest ordering and replaces stale state', () => {
 expect(traffic).toHaveProperty('mergeFlows')
 const merge=Reflect.get(traffic,'mergeFlows') as (current:traffic.Flow[],incoming:traffic.Flow[],older?:boolean)=>traffic.Flow[]
 const current=traffic.flowRows({items:[{flow_id:'old',occurred_ms:0}],next_cursor:null})
 const received=traffic.flowRows({items:[{flow_id:'new',occurred_ms:1},{flow_id:'old',occurred_ms:0,closed:{}}],next_cursor:null})
 const merged=merge(current,received)
 expect(merged.map(f=>f.flow_id)).toEqual(['new','old'])
 expect(merged[1]?.state).toBe('Closed')
 expect(merge(merged,current,true).map(f=>f.flow_id)).toEqual(['new','old'])
})

test('session rows expose both endpoints, closure and byte counters', () => {
 const rows = traffic.flowRows({items:[{flow_id:'session',service_id:'docs',occurred_ms:0,client:{ip:'192.0.2.1',port:4567},upstream:{ip:'127.0.0.1',port:8000},closed:{bytes_client_to_server:'18446744073709551615',bytes_server_to_client:'80'}}],next_cursor:null})
 expect(rows[0]).toMatchObject({client:'192.0.2.1:4567',destination:'127.0.0.1:8000',state:'Closed',bytesUp:'18446744073709551615',bytesDown:'80'})
})

test('decoded stream preserves typed scalar precision and multiline text', () => {
 expect(traffic).toHaveProperty('streamText')
 const render = Reflect.get(traffic,'streamText') as (message:{fields:Record<string,unknown>}) => string
 expect(render({fields:{'http.path':{type:'string',value:'/hello'},'protocol.text':{type:'string',value:'first\nsecond'},'protocol.count':{type:'uint',value:'18446744073709551615'}}})).toBe('http.path: /hello\nprotocol.text: first\nsecond\nprotocol.count: 18446744073709551615')
})

test('Raw profile opens the session in hex capture mode', () => {
 expect(traffic).toHaveProperty('initialStreamMode')
 const mode = Reflect.get(traffic,'initialStreamMode') as (protocolId:string)=>string
 expect(mode('raw')).toBe('bytes')
 expect(mode('utf8')).toBe('decoded')
 expect(mode('http')).toBe('decoded')
})

test('byte view preserves line breaks and displays offsets and nonprinting bytes', () => {
 expect(traffic).toHaveProperty('bytePreview')
 const render = Reflect.get(traffic,'bytePreview') as (data:number[]) => {ascii:string;hex:string;utf8:string}
 const preview=render([65,13,10,0,255,66])
 expect(preview.ascii).toBe('A\r\n··B')
 expect(preview.hex).toContain('00000000  41 0d 0a 00 ff 42')
 expect(preview.utf8).toBe('A\r\n\u0000�B')
})

test('raw byte preview keeps the complete capture in hex and UTF-8', () => {
 const render = Reflect.get(traffic,'bytePreview') as (data:number[]) => {ascii:string;hex:string;utf8:string}
 const bytes = [...new TextEncoder().encode('€'), ...new Array(5000).fill(0x61)]
 const preview=render(bytes)
 expect(preview.utf8).toBe('€'+'a'.repeat(5000))
 expect(preview.hex).toContain('00001380')
 expect(preview.hex.split('\n')).toHaveLength(Math.ceil(bytes.length/16))
})

test('stream previews are bounded and visibly report omitted content', () => {
 expect(traffic).toHaveProperty('streamText')
 const render = Reflect.get(traffic,'streamText') as (message:{fields:Record<string,unknown>}) => string
 const output=render({fields:{body:{type:'string',value:'x'.repeat(20000)}}})
 expect(output.length).toBeLessThan(8300)
 expect(output).toContain('[preview truncated]')
})

test('blocked streams are hidden by default and service colors are stable',()=>{
 const rows=traffic.flowRows({items:[{flow_id:'ok'},{flow_id:'blocked',blocked:true,has_flag:true}],next_cursor:null})
 expect(traffic.visibleFlows(rows,false).map(f=>f.flow_id)).toEqual(['ok'])
 expect(traffic.visibleFlows(rows,true)).toHaveLength(2)
 expect(traffic.serviceColor('docs')).toBe(traffic.serviceColor('docs'))
 expect(new Set(['docs','ark','crypter','aquarius'].map(traffic.serviceColor)).size).toBe(4)
 expect(traffic.serviceColor('untrusted; color:red')).toMatch(/^hsl\(\d+(?:\.\d+)? 65% 55%\)$/)
})

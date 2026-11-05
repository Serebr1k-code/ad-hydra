import {expect,test} from 'bun:test'
import {terminalMessages,terminalHistory} from './terminal'
import {messageRows,initialStreamMode} from './traffic'
import {mergeMessages} from './live'
test('UTF-8 streams default to terminal while raw still defaults to hex',()=>{
 expect(initialStreamMode('utf8')).toBe('terminal');expect(initialStreamMode('raw')).toBe('bytes');expect(initialStreamMode('http')).toBe('decoded')
})
test('terminal text uses payload values, capture order, ANSI and prompts without added field names/newlines',()=>{
 const messages=messageRows({items:[{message_id:'b',stream_sequence:2,occurred_ms:1,direction:1,fields:{'utf8.text':{type:'string',value:'name\n'}}},{message_id:'a',stream_sequence:1,occurred_ms:1,direction:2,fields:{'utf8.text':{type:'string',value:'\x1b[32mName: \x1b[0m'}}}],next_cursor:null})
 const ordered=mergeMessages([],messages)
 expect(terminalMessages(ordered,'both').chunks.map(c=>c.text).join('')).toBe('\x1b[32mName: \x1b[0mname\n')
 expect(terminalMessages(ordered,'1').chunks.map(c=>c.text)).toEqual(['name\n'])
})
test('terminal history preserves TCP interleaving and split UTF-8 independently per direction',()=>{
 const result=terminalHistory([{direction:2,sequence:1,data:[0xc3]},{direction:1,sequence:2,data:[65]},{direction:2,sequence:3,data:[0xa9,62]}],'both')
 expect(result.chunks.map(c=>c.text).join('')).toBe('Aé>');expect(result.ordered).toBe(true)
 expect(terminalHistory([{direction:1,data:[65]},{direction:2,data:[66]}],'both').ordered).toBe(false)
})
test('terminal preview stays bounded when the newest chunk fills the whole budget',()=>{
 const rows=messageRows({items:[{message_id:'a',direction:2,fields:{'utf8.text':{type:'string',value:'old'}}},{message_id:'b',direction:2,fields:{'utf8.text':{type:'string',value:'x'.repeat(256*1024)}}}],next_cursor:null})
 const result=terminalMessages(rows,'both');expect(result.chunks.map(c=>c.text).join('').length).toBe(256*1024);expect(result.truncated).toBe(true)
})
test('invalid UTF-8 renders exact hex while a real replacement character remains text',()=>{
 const result=terminalHistory([{direction:2,sequence:1,data:[65,255,0xef,0xbf,0xbd,0xc3]},{direction:1,sequence:2,data:[66]}],'both')
 expect(result.chunks.map(c=>[c.kind??'text',c.text])).toEqual([['text','A'],['hex','[ff]'],['text','�'],['text','B'],['hex','[c3]']])
})
test('live parsed invalid spans carry bytes separately from valid UTF-8 text',()=>{
 const rows=messageRows({items:[{message_id:'a',direction:2,fields:{'utf8.text':{type:'string',value:'A�B'},'utf8.spans':{type:'list',value:[{type:'string',value:'A'},{type:'bytes',value:[255]},{type:'string',value:'B'}]}}}],next_cursor:null})
 expect(terminalMessages(rows,'both').chunks.map(c=>[c.kind??'text',c.text])).toEqual([['text','A'],['hex','[ff]'],['text','B']])
})

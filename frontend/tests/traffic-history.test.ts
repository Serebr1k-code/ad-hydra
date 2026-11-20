import { expect, test } from 'bun:test'
import { historyPreview, historySegments, historyTextSegments, historyUtf8Segments, onlyRawSummaryMessages, messageRows } from '../src/features/traffic/traffic'

test('closed history keeps client and server directions distinct', () => {
 const history={segments:[{direction:1,data:[0x41]},{direction:2,data:[0x42]}],truncated_client_to_server:true,truncated_server_to_client:false}
 expect(historySegments(history,'both').map(s=>s.direction)).toEqual([1,2])
 expect(historySegments(history,'1').map(s=>s.data)).toEqual([[0x41]])
 expect(historySegments(history,'2').map(s=>s.data)).toEqual([[0x42]])
 expect(historySegments(history,'1')[0]?.truncated).toBe(true)
 expect(historySegments(history,'2')[0]?.truncated).toBe(false)
})

test('raw history defaults to hex while UTF-8 decoding replaces invalid bytes', () => {
 const segment={direction:1,data:[0x41,0xff,0x42]}
 expect(historyPreview(segment,'hex')).toContain('41 ff 42')
 expect(historyPreview(segment,'utf8')).toBe('A�B')
 expect(historyPreview(segment,'ascii')).toBe('A·B')
})

test('raw UTF-8 fallback groups retained chunks by direction before decoding', () => {
 const history={segments:[{direction:1,data:[0xc3]},{direction:2,data:[0x42]},{direction:1,data:[0xa9]}],truncated_client_to_server:false,truncated_server_to_client:false}
 expect(historyTextSegments(history,'both').map(segment=>[segment.direction,historyPreview(segment,'utf8')])).toEqual([[1,'é'],[2,'B']])
 expect(historyTextSegments(history,'1').map(segment=>[segment.direction,historyPreview(segment,'utf8')])).toEqual([[1,'é']])
})

test('ordered TCP UTF-8 fallback preserves cross-direction capture order and split code points', () => {
 const history={segments:[
  {direction:1,data:[0x41,0xc3],sequence:1},
  {direction:2,data:[0x42],sequence:2},
  {direction:1,data:[0xa9,0x43],sequence:3},
  {direction:2,data:[0x44],sequence:4},
 ],truncated_client_to_server:false,truncated_server_to_client:false}
 const segments=historyTextSegments(history,'both')
 expect(segments.map(s=>s.direction)).toEqual([1,2,1,2])
 expect(segments.map(s=>s.previewText)).toEqual(['A','B','éC','D'])
 expect(segments.map(s=>s.data)).toEqual([[0x41,0xc3],[0x42],[0xa9,0x43],[0x44]])
 expect(segments.map(s=>s.sequence)).toEqual([1,2,3,4])
 expect(historyTextSegments(history,'1').map(s=>s.previewText)).toEqual(['A','éC'])
})

test('captured-byte UTF-8 view decodes split TCP code points without changing raw bytes',()=>{
 const history={segments:[{direction:1,data:[0x41,0xc3],sequence:1},{direction:2,data:[0x42],sequence:2},{direction:1,data:[0xa9],sequence:3}],truncated_client_to_server:false,truncated_server_to_client:false}
 const segments=historyUtf8Segments(history,'both')
 expect(segments.map(s=>s.previewText)).toEqual(['A','B','é'])
 expect(segments.map(s=>s.data)).toEqual([[0x41,0xc3],[0x42],[0xa9]])
})

test('only native raw byte summaries are eligible for saved text fallback',()=>{
 const summary=messageRows({items:[{message_id:'raw-summary',protocol_id:'transfer',message_type:'byte_summary'}],next_cursor:null})
 const wasm=messageRows({items:[{message_id:'wasm-summary',protocol_id:'browser-wasm',message_type:'byte_summary'}],next_cursor:null})
 const decoder=messageRows({items:[{message_id:'raw-decoder',protocol_id:'raw',message_type:'bytes'}],next_cursor:null})
 expect(onlyRawSummaryMessages(summary,'raw')).toBe(true)
 expect(onlyRawSummaryMessages(summary,'custom:browser-wasm/v1')).toBe(false)
 expect(onlyRawSummaryMessages(wasm,'custom:browser-wasm/v1')).toBe(false)
 expect(onlyRawSummaryMessages([...summary,...decoder],'raw')).toBe(false)
 expect(onlyRawSummaryMessages([],'raw')).toBe(false)
})


test('UDP text keeps binary and empty datagrams in captured order', () => {
 const history={segments:[
  {direction:1,data:[0xc3],datagram:true,sequence:1,occurred_at_ms:10},
  {direction:2,data:[],datagram:true,sequence:2,occurred_at_ms:11},
  {direction:1,data:[0xa9,0],datagram:true,sequence:3,occurred_at_ms:12},
 ],truncated_client_to_server:false,truncated_server_to_client:false}
 const segments=historyTextSegments(history,'both')
 expect(segments.map(s=>[s.direction,s.sequence,s.occurred_at_ms,s.data])).toEqual([[1,1,10,[0xc3]],[2,2,11,[]],[1,3,12,[0xa9,0]]])
 expect(segments.map(s=>historyPreview(s,'utf8'))).toEqual(['�','','�\0'])
 expect(segments.map(s=>s.previewText)).toEqual(['�','','�\0'])
 expect(historySegments(history,'1').map(s=>s.sequence)).toEqual([1,3])
})

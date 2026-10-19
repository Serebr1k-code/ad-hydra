import { expect, test } from 'bun:test'
import { historyPreview, historySegments, historyTextSegments } from '../src/features/traffic/traffic'

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


test('UDP text keeps binary and empty datagrams in captured order', () => {
 const history={segments:[
  {direction:1,data:[0xc3],datagram:true,sequence:1,occurred_at_ms:10},
  {direction:2,data:[],datagram:true,sequence:2,occurred_at_ms:11},
  {direction:1,data:[0xa9,0],datagram:true,sequence:3,occurred_at_ms:12},
 ],truncated_client_to_server:false,truncated_server_to_client:false}
 const segments=historyTextSegments(history,'both')
 expect(segments.map(s=>[s.direction,s.sequence,s.occurred_at_ms,s.data])).toEqual([[1,1,10,[0xc3]],[2,2,11,[]],[1,3,12,[0xa9,0]]])
 expect(segments.map(s=>historyPreview(s,'utf8'))).toEqual(['�','','�\0'])
 expect(historySegments(history,'1').map(s=>s.sequence)).toEqual([1,3])
})

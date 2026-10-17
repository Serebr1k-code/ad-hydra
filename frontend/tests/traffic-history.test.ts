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

import { describe, expect, test } from 'bun:test'
import { EventFrames, isEventFrame } from '../src/shared/api/sse'
import { normalizeApiBaseUrl } from '../src/shared/config/env'
import { path } from '../src/generated/operations'
import { differences, fields } from '../src/features/traffic/traffic'
describe('bounded live metadata', () => {
 test('retains fragments and splits complete batches', () => {
  const frames=new EventFrames()
  expect(frames.push('event: message\ndata: {')).toEqual([])
  expect(frames.push('}\n\nevent: loss\ndata: 2\n\n')).toEqual(['event: message\ndata: {}','event: loss\ndata: 2'])
 })
 test('rejects an oversized incomplete and complete frame', () => {
  expect(() => new EventFrames().push('x'.repeat(262145))).toThrow('limit')
  expect(() => new EventFrames().push('x'.repeat(262145)+'\n\n')).toThrow('limit')
 })
})
test('API build prefix is normalized and generated path values are encoded', () => {
 expect(normalizeApiBaseUrl()).toBe('/api')
 expect(normalizeApiBaseUrl(' https://management.example/api/// ')).toBe('https://management.example/api')
 expect(path('get_filter',{id:'a/b'})).toBe('/api/v1/filters/a%2Fb')
})
test('conversation comparison preserves full-width integer strings and missing fields', () => {
 const a={message_id:'a',fields:{count:{type:'uint',value:'18446744073709551615'},removed:{type:'bool',value:true}}}
 const b={message_id:'b',fields:{count:{type:'uint',value:'18446744073709551614'},added:{type:'string',value:'new'}}}
 expect(fields(a)[0]?.value).toBe('"18446744073709551615"')
 expect(differences(a,b).map(v=>[v.key,v.change])).toEqual([['added','Added'],['count','Changed'],['removed','Removed']])
})

test('keepalive comments do not announce traffic events', () => {
 const frames = new EventFrames()
 const delivered = frames.push(': keepalive\n\nevent: traffic\ndata: {}\n\n: another keepalive\n\nevent: loss\ndata: 2\n\n').filter(isEventFrame)
 expect(delivered).toEqual(['event: traffic\ndata: {}','event: loss\ndata: 2'])
 expect(isEventFrame(': event: traffic')).toBe(false)
 expect(isEventFrame('data: {"event":"traffic"}')).toBe(false)
})

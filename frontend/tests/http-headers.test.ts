import { expect, test } from 'bun:test'
import { httpHeaderLines, streamText } from '../src/features/traffic/traffic'
const header=(name:string,value:string)=>({type:'object',value:{name:{type:'string',value:name},value:{type:'string',value}}})
const list=(value:unknown[])=>({type:'list',value})

test('HTTP header lines preserve duplicates, original names, whitespace and empty values',()=>{
 const headers=list([header('Set-Cookie',' a=one'),header('set-cookie','\tb=two '),header('X-Empty','')])
 expect(httpHeaderLines(headers)).toEqual(['Set-Cookie: a=one','set-cookie:\tb=two ','X-Empty:'])
 expect(streamText({fields:{'http.headers':headers}})).toBe('http.headers:\nSet-Cookie: a=one\nset-cookie:\tb=two \nX-Empty:')
})

test('all bounded HTTP headers remain visible even after other fields exhaust preview',()=>{
 const headers=list([header('X-Large','a'.repeat(16000)),header('X-Last',' visible')])
 const output=streamText({fields:{body:{type:'string',value:'b'.repeat(10000)},'http.headers':headers}})
 expect(output).toContain('[preview truncated]')
 expect(output).toContain('X-Large:'+'a'.repeat(16000))
 expect(output).toContain('X-Last: visible')
 expect(output).not.toContain('[headers truncated]')
 expect(output.length).toBeLessThan(25000)
})

test('malformed and excessive header lists are safely bounded',()=>{
 expect(httpHeaderLines({type:'string',value:'unstructured'})).toBeUndefined()
 expect(httpHeaderLines(list([null,header('X',' valid')]))).toEqual(['X: valid'])
 expect(httpHeaderLines(list([header('X','a'.repeat(16384))]))).toEqual(['[headers truncated]'])
 expect(httpHeaderLines(list(Array.from({length:129},()=>header('X','v'))))?.at(-1)).toBe('[headers truncated]')
})

test('response trailers render ordered header lines alongside initial headers',()=>{
 const fields={'http.headers':list([header('Trailer',' X-Result')]),'http.trailers':list([header('X-Result',' one'),header('x-result',' two')])}
 expect(streamText({fields})).toBe('http.headers:\nTrailer: X-Result\nhttp.trailers:\nX-Result: one\nx-result: two')
})

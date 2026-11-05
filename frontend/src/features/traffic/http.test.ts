import {test,expect} from 'bun:test'
import {httpMessage,isHttpMessage} from './http'
import {messageRows} from './traffic'
const str=(value:string)=>({type:'string',value})
const headers=(values:[string,string][])=>({type:'list',value:values.map(([name,value])=>({type:'object',value:{name:str(name),value:str(value)}}))})
test('HTTP request view preserves duplicate headers and parses repeated query parameters',()=>{
 const message=messageRows({items:[{message_id:'r',direction:1,fields:{'http.method':str('POST'),'http.path':str('/users?a=one&a=two&x=%3Cscript%3E'),'http.headers':headers([['X-Test',' first'],['X-Test','\tsecond'],['Content-Type',' application/json']]),'http.body_text':str('{"ok":true}'),'http.body_length':{type:'uint',value:'11'}}}],next_cursor:null})[0]!
 const view=httpMessage(message)!
 expect(isHttpMessage(message)).toBe(true);expect(view.title).toBe('POST /users?a=one&a=two&x=%3Cscript%3E');expect(view.headers.items.slice(0,2)).toEqual([{name:'X-Test',value:' first'},{name:'X-Test',value:'\tsecond'}]);expect(view.query.items).toEqual([{name:'a',value:'one'},{name:'a',value:'two'},{name:'x',value:'<script>'}]);expect(view.prettyBody).toBe('{\n  "ok": true\n}');expect(view.body).toBe('{"ok":true}')
})
test('response view exposes status, trailers and missing or truncated text honestly',()=>{
 const message=messageRows({items:[{message_id:'s',direction:2,fields:{'http.status_code':{type:'uint',value:'404'},'http.reason':str('Not Found'),'http.body_length':{type:'uint',value:'9007199254740993'},'http.body_preview_truncated':{type:'bool',value:true},'http.trailers':headers([['Checksum',' value']])}}],next_cursor:null})[0]!
 const view=httpMessage(message)!
 expect(view.title).toBe('404 Not Found');expect(view.body).toBeUndefined();expect(view.bodyLength).toBe('9007199254740993');expect(view.truncated).toBe(true);expect(view.trailers.items[0]!.name).toBe('Checksum')
 expect(httpMessage(messageRows({items:[{fields:{'utf8.text':str('hello')}}],next_cursor:null})[0]!)).toBeUndefined()
})
test('pretty JSON and query/header previews are bounded',()=>{
 const fields={'http.method':str('GET'),'http.path':str('/?'+Array.from({length:200},(_,i)=>'a='+i).join('&')),'http.headers':headers(Array.from({length:200},()=>['X',' value'])),'http.body_text':str('['.repeat(100)+'0'+']'.repeat(100))}
 const view=httpMessage(messageRows({items:[{fields}],next_cursor:null})[0]!)!
 expect(view.query.items.length).toBe(128);expect(view.query.truncated).toBe(true);expect(view.headers.items.length).toBe(128);expect(view.headers.truncated).toBe(true);expect(view.prettyBody).toBeUndefined()
})
test('JSON formatting preserves large numbers, duplicate keys and original escapes',()=>{
 const body='{"id":9007199254740993,"x":1,"x":2,"n":1e+09,"negative":-0,"escaped":"\\u0061","braces":"[{}]"}'
 const view=httpMessage(messageRows({items:[{fields:{'http.method':str('POST'),'http.body_text':str(body),'http.headers':headers([['Content-Type',' application/json']])}}],next_cursor:null})[0]!)!
 expect(view.prettyBody).toBe('{\n  "id": 9007199254740993,\n  "x": 1,\n  "x": 2,\n  "n": 1e+09,\n  "negative": -0,\n  "escaped": "\\u0061",\n  "braces": "[{}]"\n}')
 expect(view.body).toBe(body)
})

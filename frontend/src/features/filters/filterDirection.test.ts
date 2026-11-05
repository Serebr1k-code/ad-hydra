import {expect,test} from 'bun:test'
import {withDirection,splitDirection,directionLabel} from './filterDirection'
const expression={any:[{field:'utf8.text',op:'contains',value:{type:'string',value:'hello'}},{not:{field:'raw.bytes',op:'eq',value:{type:'bytes',value:[255]}}}]}
test('direction restricts the entire expression and both preserves existing rules',()=>{
 expect(withDirection(expression,'both')).toEqual(expression)
 for(const side of ['client_to_server','server_to_client'] as const){
  expect(withDirection(expression,side)).toEqual({all:[{field:'direction',op:'eq',value:{type:'string',value:side}},expression]})
  expect(splitDirection(withDirection(expression,side))).toEqual({side,expression})
 }
})
test('edit/save round trips direction without adding nested wrappers',()=>{
 const first=withDirection(expression,'server_to_client');const restored=splitDirection(first)
 expect(withDirection(restored.expression,restored.side)).toEqual(first)
 expect(withDirection(restored.expression,'client_to_server')).toEqual(withDirection(expression,'client_to_server'))
 expect(withDirection(restored.expression,'both')).toEqual(expression)
})
test('legacy predicates, OR and negations keep their original meaning',()=>{
 for(const legacy of [expression,{not:withDirection(expression,'client_to_server')},{any:[{field:'direction',op:'eq',value:{type:'string',value:'client_to_server'}},expression]}]){
  expect(splitDirection(legacy)).toEqual({side:'both',expression:legacy})
 }
 expect(directionLabel('server_to_client')).toBe('Server → client');expect(directionLabel('both')).toBe('Both directions')
})

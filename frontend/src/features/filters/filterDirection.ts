import type {Expression} from './ruleBuilder'
export type FilterDirection='both'|'client_to_server'|'server_to_client'
export function directionLabel(side:FilterDirection):string{return side==='client_to_server'?'Client → server':side==='server_to_client'?'Server → client':'Both directions'}
export function withDirection(expression:Expression,side:FilterDirection):Expression{
 return side==='both'?expression:{all:[{field:'direction',op:'eq',value:{type:'string',value:side}},expression]}
}
/** Extract only the canonical outer guard; never lift conditions out of OR/NOT groups. */
export function splitDirection(expression:Expression):{side:FilterDirection;expression:Expression}{
 const all=expression.all
 if(Array.isArray(all)&&all.length===2&&all[0]&&typeof all[0]==='object'&&all[1]&&typeof all[1]==='object'){
  const guard=all[0] as Record<string,unknown>,value=guard.value as Record<string,unknown>|undefined
  if(guard.field==='direction'&&guard.op==='eq'&&value?.type==='string'&&(value.value==='client_to_server'||value.value==='server_to_client'))return {side:value.value,expression:all[1] as Expression}
 }
 return {side:'both',expression}
}

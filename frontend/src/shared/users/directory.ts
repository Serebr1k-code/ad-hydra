import {shallowRef} from 'vue'
import type {User} from '@/generated/contracts'
import {api} from '@/shared/api/client'
export const userNames=shallowRef<Record<string,string>>({})
let generation=0
const userFields=new Set(['author_id','actor_id','initiated_by','created_by','updated_by','user_id'])
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
export function setUserDirectory(users:Pick<User,'id'|'username'>[]):void {generation++;userNames.value=Object.fromEntries(users.map(user=>[user.id,user.username]))}
export function clearUserDirectory():void {generation++;userNames.value={}}
export async function loadUserDirectory():Promise<void>{const current=++generation;const users=await api<User[]>('/users');if(current===generation)userNames.value=Object.fromEntries(users.map(user=>[user.id,user.username]))}
export function username(id:unknown):string {
 if(typeof id!=='string'||!id)return '—'
 return userNames.value[id]??(uuid.test(id)?'Unknown user':id)
}
export function identityMetadata(value:unknown):unknown {
 if(Array.isArray(value))return value.map(identityMetadata)
 if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,userFields.has(key)?username(item):identityMetadata(item)]))
 return value
}
export function displayCell(value:unknown,key:string):string {
 if(userFields.has(key))return username(value)
 if(key==='target'&&typeof value==='string'&&userNames.value[value])return userNames.value[value]
 if(key==='metadata'){
  try{return JSON.stringify(identityMetadata(typeof value==='string'?JSON.parse(value):value))}catch{/* Preserve non-JSON audit text. */}
 }
 if(value===null||value===undefined)return '—'
 return typeof value==='object'?JSON.stringify(value):String(value)
}

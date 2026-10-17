import {afterEach,expect,test} from 'bun:test'
import {clearUserDirectory,displayCell,setUserDirectory,username} from './directory'
const id='9de9043e-c83c-423e-a85c-2783e35970f3'
afterEach(clearUserDirectory)
test('user labels resolve across provenance columns, audit target and nested metadata without replacing technical IDs',()=>{
 setUserDirectory([{id,username:'operator'}])
 for(const field of ['created_by','updated_by','author_id','actor_id','initiated_by','user_id'])expect(displayCell(id,field)).toBe('operator')
 expect(displayCell(id,'target')).toBe('operator')
 expect(displayCell(JSON.stringify({initiated_by:id,before:{user_id:id}}),'metadata')).toBe('{"initiated_by":"operator","before":{"user_id":"operator"}}')
 expect(displayCell(id,'request_id')).toBe(id)
 expect(displayCell(id,'value')).toBe(id)
 setUserDirectory([{id,username:'renamed'}]);expect(username(id)).toBe('renamed')
 clearUserDirectory();expect(username(id)).toBe('Unknown user');expect(username('proxy')).toBe('proxy')
})

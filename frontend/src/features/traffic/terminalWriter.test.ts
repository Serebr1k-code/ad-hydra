import {test,expect} from 'bun:test'
import {TranscriptWriter} from './terminalWriter'
test('direction/history replacement waits for queued terminal parsing, then replaces without stale text',()=>{
 let screen='';const pending:(()=>void)[]=[]
 const writer=new TranscriptWriter({reset:()=>{screen=''},write:(text,done)=>{pending.push(()=>{screen+=text;done()})}})
 writer.set([{id:'old',direction:1,text:'client'}]);writer.set([{id:'new',direction:2,text:'server'}]);
 pending.shift()!();pending.shift()!();expect(screen).toBe('\x1b[38;2;96;165;250mserver')
})
test('slow terminal rendering coalesces updates and disposal ignores late callbacks',()=>{
 let screen='';const pending:(()=>void)[]=[];let complete=0
 const writer=new TranscriptWriter({reset:()=>{screen=''},write:(text,done)=>{pending.push(()=>{screen+=text;done()})}},()=>{complete++})
 writer.set([{id:'a',direction:2,text:'a'}]);writer.set([{id:'b',direction:2,text:'b'}]);writer.set([{id:'c',direction:2,text:'c'}]);pending.shift()!();pending.shift()!();expect(screen).toBe('\x1b[38;2;96;165;250mc');expect(pending.length).toBe(0)
 writer.set([{id:'d',direction:2,text:'d'}]);writer.dispose();pending.shift()!();expect(complete).toBe(2)
})

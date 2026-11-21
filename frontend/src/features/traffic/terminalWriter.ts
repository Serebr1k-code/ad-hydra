import type {TerminalChunk} from './terminal'
import {coloredChunks,flagChunks} from './terminalColors'
interface Output {reset():void;write(text:string,done:()=>void):void}
/** One in-flight parser write and one coalesced desired transcript. Reset only after parsing drains. */
export class TranscriptWriter {
 private written:TerminalChunk[]=[]
 private pending:TerminalChunk[]|undefined
 private writing=false
 private disposed=false
 constructor(private output:Output,private completed:()=>void=()=>{},private flags:()=>RegExp|null=()=>null){}
 set(chunks:TerminalChunk[]){if(this.disposed)return;this.pending=flagChunks(coloredChunks(chunks),this.flags());this.flush()}
 dispose(){this.disposed=true;this.pending=undefined}
 private flush(){
  if(this.disposed||this.writing||!this.pending)return
  const next=this.pending;this.pending=undefined
  const append=this.written.length<=next.length&&this.written.every((old,index)=>old.id===next[index]?.id&&old.text===next[index]?.text)
  const start=append?this.written.length:0;if(!append)this.output.reset()
  const text=next.slice(start).map(chunk=>chunk.text).join('');this.written=next.map(c=>({...c}))
  if(!text){this.completed();return}
  this.writing=true
  this.output.write(text,()=>{if(this.disposed)return;this.writing=false;this.completed();this.flush()})
 }
}
